use std::{marker::PhantomData, pin::Pin};

use async_stream::stream;
use futures_util::{stream::Stream, StreamExt};
use raw::{ClientMessage, ClientPayload, GraphQLReceiver, GraphQLSender, Payload, ServerMessage};
use serde::de::DeserializeOwned;
use tokio::sync::broadcast;
use tokio_tungstenite::connect_async;

pub mod raw;

pub use tungstenite::handshake::client::Request;

pub struct GraphQLWebSocket {
    tx: broadcast::Sender<ClientMessage>,
    server_tx: broadcast::Sender<ServerMessage>,
    #[allow(dead_code)] // Need this to avoid a hangup
    server_rx: broadcast::Receiver<ServerMessage>,
    id_count: u64,
}

impl GraphQLWebSocket {
    pub async fn connect(request: Request) -> Result<GraphQLWebSocket, tungstenite::Error> {
        let (stream, _) = match connect_async(request).await {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        let (sink, stream) = StreamExt::split(stream);

        let (tx_in, rx_in) = broadcast::channel(16);

        let tx_in0 = tx_in.clone();
        tokio::spawn(async move {
            let rx = GraphQLReceiver { stream };
            let mut stream = rx.stream();
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(ServerMessage::ConnectionKeepAlive) => {}
                    Ok(v) => {
                        let _ = tx_in0.send(v);
                    }
                    Err(e) => tracing::error!("{:?}", e),
                }
            }
        });

        let (tx_out, mut rx_out) = broadcast::channel(16);
        tokio::spawn(async move {
            let mut tx = GraphQLSender { sink };

            tx.send(ClientMessage::ConnectionInit { payload: None })
                .await
                .unwrap();

            while let Ok(msg) = rx_out.recv().await {
                match tx.send(msg).await {
                    Ok(()) => {}
                    Err(e) => tracing::error!("{:?}", e),
                }
            }
        });

        let socket = GraphQLWebSocket {
            tx: tx_out,
            server_tx: tx_in,
            server_rx: rx_in,
            id_count: 0,
        };

        Ok(socket)
    }

    pub fn subscribe<T: DeserializeOwned + Unpin + Send + 'static>(
        &mut self,
        payload: ClientPayload,
    ) -> GraphQLSubscription<T> {
        self.id_count += 1;
        let id = format!("{:x}", self.id_count);

        let sub =
            GraphQLSubscription::<T>::new(id, self.tx.clone(), self.server_tx.subscribe(), payload);

        sub
    }
}

pub struct GraphQLSubscription<
    T: DeserializeOwned = serde_json::Value,
    E: DeserializeOwned = serde_json::Value,
> {
    id: String,
    tx: broadcast::Sender<ClientMessage>,
    rx: broadcast::Receiver<ServerMessage>,
    payload: ClientPayload,
    ty_value: PhantomData<T>,
    ty_error: PhantomData<E>,
}

pub enum SubscriptionError {
    InvalidData(Payload),
    InternalError(serde_json::Value),
}

impl<T, E> GraphQLSubscription<T, E>
where
    T: DeserializeOwned + Unpin + Send + 'static,
    E: DeserializeOwned + Unpin + Send + 'static,
{
    pub fn new(
        id: String,
        tx: broadcast::Sender<ClientMessage>,
        rx: broadcast::Receiver<ServerMessage>,
        payload: ClientPayload,
    ) -> Self {
        Self {
            id,
            tx,
            rx,
            payload,
            ty_value: PhantomData,
            ty_error: PhantomData,
        }
    }

    pub fn stream(
        self,
    ) -> Pin<Box<dyn Stream<Item = Result<Payload<T, E>, serde_json::Value>> + Send>> {
        let mut this = self;

        Box::pin(stream! {
            this.tx.send(ClientMessage::Start {
                id: this.id.clone(),
                payload: this.payload.clone(),
            }).unwrap();

            while let Ok(msg) = this.rx.recv().await {
                match msg {
                    ServerMessage::Data { id, payload } => {
                        if id == this.id {
                            let raw_data = payload.data.unwrap_or(serde_json::Value::Null);
                            let raw_errors = payload.errors.unwrap_or(serde_json::Value::Null);

                            let data: Option<T> = serde_json::from_value(raw_data).unwrap_or(None);
                            let errors: Option<E> = serde_json::from_value(raw_errors).unwrap_or(None);

                            yield Ok(Payload { data, errors });
                        }
                    }
                    ServerMessage::Complete { id } => {
                        if id == this.id {
                            return;
                        }
                    }
                    ServerMessage::ConnectionError { payload } => {
                        yield Err(payload);
                        return;
                    }
                    ServerMessage::Error { id, payload } => {
                        if id == this.id {
                            yield Err(payload);
                        }
                    }
                    ServerMessage::ConnectionAck => {}
                    ServerMessage::ConnectionKeepAlive => {}
                }
            }
        })
    }
}

impl<T, E> Drop for GraphQLSubscription<T, E>
where
    T: DeserializeOwned,
    E: DeserializeOwned,
{
    fn drop(&mut self) {
        self.tx
            .send(ClientMessage::Stop {
                id: self.id.clone(),
            })
            .unwrap_or(0);
    }
}

impl Drop for GraphQLWebSocket {
    fn drop(&mut self) {
        self.tx
            .send(ClientMessage::ConnectionTerminate)
            .unwrap_or(0);
    }
}
