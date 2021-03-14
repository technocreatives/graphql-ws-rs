use std::convert::TryFrom;

use futures_util::stream::{SplitSink, SplitStream, Stream};
use futures_util::{pin_mut, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    tungstenite::protocol, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename = "camelCase")]
pub struct ClientPayload {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename = "camelCase")]
pub struct Payload<T = serde_json::Value, E = serde_json::Value> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<E>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "connection_init")]
    ConnectionInit {
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },

    #[serde(rename = "start")]
    Start { id: String, payload: ClientPayload },

    #[serde(rename = "stop")]
    Stop { id: String },

    #[serde(rename = "connection_terminate")]
    ConnectionTerminate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "error")]
    ConnectionError { payload: serde_json::Value },

    #[serde(rename = "connection_ack")]
    ConnectionAck,

    #[serde(rename = "data")]
    Data { id: String, payload: Payload },

    #[serde(rename = "error")]
    Error {
        id: String,
        payload: serde_json::Value,
    },

    #[serde(rename = "complete")]
    Complete { id: String },

    #[serde(rename = "ka")]
    ConnectionKeepAlive,
}

impl ServerMessage {
    pub fn id(&self) -> Option<&str> {
        match self {
            ServerMessage::Data { id, .. } => Some(&id),
            ServerMessage::Error { id, .. } => Some(&id),
            ServerMessage::Complete { id } => Some(&id),
            _ => None,
        }
    }
}

impl From<ClientMessage> for protocol::Message {
    fn from(message: ClientMessage) -> Self {
        Message::Text(serde_json::to_string(&message).unwrap())
    }
}

#[derive(Debug)]
pub enum MessageError {
    Decoding(serde_json::Error),
    InvalidMessage(protocol::Message),
    WebSocket(tokio_tungstenite::tungstenite::Error),
}

impl TryFrom<protocol::Message> for ServerMessage {
    type Error = MessageError;

    fn try_from(value: protocol::Message) -> Result<Self, MessageError> {
        match value {
            Message::Text(value) => {
                serde_json::from_str(&value).map_err(|e| MessageError::Decoding(e))
            }
            _ => Err(MessageError::InvalidMessage(value)),
        }
    }
}

pub struct GraphQLSender<S>
where
    S: AsyncWrite + Unpin,
{
    pub(crate) sink: SplitSink<WebSocketStream<MaybeTlsStream<S>>, Message>,
}

impl<S> GraphQLSender<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn send(
        &mut self,
        message: ClientMessage,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        let sink = &mut self.sink;
        pin_mut!(sink);
        SinkExt::send(&mut sink, message.into()).await
    }
}

pub struct GraphQLReceiver<S>
where
    S: AsyncRead + Unpin + Send,
{
    pub(crate) stream: SplitStream<WebSocketStream<MaybeTlsStream<S>>>,
}

impl<S> GraphQLReceiver<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    pub fn stream(self) -> impl Stream<Item = Result<ServerMessage, MessageError>> + Send {
        StreamExt::map(self.stream, |x| {
            tracing::trace!("{:?}", &x);

            match x {
                Ok(msg) => ServerMessage::try_from(msg),
                Err(e) => Err(MessageError::WebSocket(e)),
            }
        })
    }
}
