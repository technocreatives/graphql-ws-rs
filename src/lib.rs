use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use async_stream::stream;
use futures_util::{sink::Sink, stream::Stream, StreamExt};
use raw::{ClientMessage, ClientPayload, GraphQLReceiver, GraphQLSender, Payload, ServerMessage};
use serde::{de::DeserializeOwned, Serialize};
use tokio::{io::{AsyncRead, AsyncWrite}, sync::{RwLock, broadcast, oneshot}};

pub mod raw;

use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
pub use tungstenite::handshake::client::Request;
use tungstenite::Message;

pub struct GraphQLWebSocket {
    tx: broadcast::Sender<ClientMessage>,
    server_tx: broadcast::Sender<ServerMessage>,
    server_rx: broadcast::Receiver<ServerMessage>,
    id_count: u64,
}

impl GraphQLWebSocket {
    pub async fn connect(request: Request) -> Result<GraphQLWebSocket, tungstenite::Error> {
        let (stream, _) = match connect_async(request).await {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        let (
            sink,
            stream
        ) = StreamExt::split(stream);

        let (tx_in, rx_in) = broadcast::channel(16);

        let tx_in0 = tx_in.clone();
        tokio::spawn(async move {
            let rx = GraphQLReceiver { stream };
            let mut stream = rx.stream();
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(ServerMessage::ConnectionKeepAlive) => {},
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
            while let Ok(msg) = rx_out.recv().await {
                match tx.send(msg).await {
                    Ok(()) => {},
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

    pub fn subscribe<T: DeserializeOwned + Unpin>(
        &mut self,
        payload: ClientPayload,
    ) -> GraphQLSubscription<T> {
        self.id_count += 1;
        let id = self.id_count.to_string();

        let sub = GraphQLSubscription::<T>::new(
            id,
            self.tx.clone(),
            self.server_tx.subscribe(),
            payload,
        );

        // let mut rx = self.tx.subscribe();

        // let id = id0.clone();
        // let stream = stream! {
        //     while let Ok(msg) = rx.recv().await {
        //         if let Some(msg_id) = msg.id() {
        //             if id == msg_id {
        //                 yield msg
        //             }
        //         }
        //     }
        // };

        // self.gql_tx
        //     .send(ClientMessage::Start { id: id0, payload })
        //     .await
        //     .unwrap();

        // stream

        sub
    }
}

pub struct GraphQLSubscription<T: DeserializeOwned> {
    id: String,
    tx: broadcast::Sender<ClientMessage>,
    rx: broadcast::Receiver<ServerMessage>,
    payload: ClientPayload,
    ty: PhantomData<T>,
}

pub enum SubscriptionError {
    InvalidData(Payload),
    InternalError(serde_json::Value),
}

impl<T> GraphQLSubscription<T> where T: DeserializeOwned + Unpin {
    pub fn new(id: String, tx: broadcast::Sender<ClientMessage>, rx: broadcast::Receiver<ServerMessage>, payload: ClientPayload) -> Self {
        Self {
            id,
            tx,
            rx,
            payload,
            ty: PhantomData,
        }
    }

    // pub fn stream(self) -> impl Stream<Item = Result<Payload<T>, SubscriptionError>> {
    //     stream! {
    //         let mut s = self.raw_stream();
    //         while let Some(result) = StreamExt::next(&mut s).await {
    //             match result {
    //                 Ok(v) => { todo!(); }
    //                 Err(e) => { yield Err(SubscriptionError::InternalError(e)) }
    //             }
    //         }
    //     }
    // }

    // Payload has data and errors, need to return a Result type for this.
    // Result<Result<T, Vec<Error>>, Error>

    pub fn raw_stream(self) -> impl Stream<Item = Result<Payload, serde_json::Value>> {
        let mut this = self;

        stream! {
            this.tx.send(ClientMessage::Start {
                id: this.id.clone(),
                payload: this.payload.clone(),
            }).unwrap();

            while let Ok(msg) = this.rx.recv().await {
                match msg {
                    ServerMessage::Data { id, payload } => {
                        if id == this.id {
                            yield Ok(payload);
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
        }
    }
}

impl<T> Drop for GraphQLSubscription<T> where T: DeserializeOwned {
    fn drop(&mut self) {
        self.tx
            .send(ClientMessage::Stop { id: self.id.clone() })
            .unwrap_or(0);
    }
}
