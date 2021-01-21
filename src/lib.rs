use std::convert::TryFrom;

use futures_util::{SinkExt, StreamExt, pin_mut};
use futures_util::{
    stream::{SplitSink, SplitStream, Stream},
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::protocol, WebSocketStream};
pub use tungstenite::handshake::client::Request;
use tungstenite::Message;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "camelCase")]
pub struct ClientPayload {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "camelCase")]
pub struct Payload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]

pub enum ClientMessage {
    #[serde(rename = "connection_init")]
    ConnectionInit {
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>
    },

    #[serde(rename = "start")]
    Start { id: String, payload: ClientPayload },

    #[serde(rename = "stop")]
    Stop { id: String },

    #[serde(rename = "connection_terminate")]
    ConnectionTerminate,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "error")]
    ConnectionError {
        payload: serde_json::Value,
    },

    #[serde(rename = "connection_ack")]
    ConnectionAck,

    #[serde(rename = "data")]
    Data {
        id: String,
        payload: Payload,
    },

    #[serde(rename = "error")]
    Error {
        id: String,
        payload: serde_json::Value,
    },

    #[serde(rename = "complete")]
    Complete {
        id: String,
    },

    #[serde(rename = "ka")]
    ConnectionKeepAlive,
}

impl From<ClientMessage> for protocol::Message {
    fn from(message: ClientMessage) -> Self {
        dbg!(Message::Text(serde_json::to_string(&message).unwrap()))
    }
}

#[derive(Debug)]
pub enum MessageError {
    Decoding(serde_json::Error),
    InvalidMessage(protocol::Message),
    WebSocket(tungstenite::Error),
}

impl TryFrom<protocol::Message> for ServerMessage {
    type Error = MessageError;

    fn try_from(value: protocol::Message) -> Result<Self, MessageError> {
        match value {
            Message::Text(value) => {
                dbg!(serde_json::from_str(&value).map_err(|e| MessageError::Decoding(e)))
            }
            _ => Err(MessageError::InvalidMessage(value)),
        }
    }
}

pub struct GraphQLSender<S> {
    sink: SplitSink<WebSocketStream<S>, Message>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> GraphQLSender<S> {
    pub async fn send(&mut self, message: ClientMessage) -> Result<(), tungstenite::Error> {
        let sink = &mut self.sink;
        pin_mut!(sink);
        sink.send(message.into()).await
    }
}

pub struct GraphQLReceiver<S> {
    stream: SplitStream<WebSocketStream<S>>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> GraphQLReceiver<S> {
    pub fn stream(self) -> impl Stream<Item = Result<ServerMessage, MessageError>> {
        self.stream.map(|x| {
            match dbg!(x) {
                Ok(msg) => ServerMessage::try_from(msg),
                Err(e) => Err(MessageError::WebSocket(e)),
            }
        })
    }

    pub fn into_inner(self) -> SplitStream<WebSocketStream<S>> {
        self.stream
    }
}

pub fn split<S>(stream: WebSocketStream<S>) -> (GraphQLSender<S>, GraphQLReceiver<S>)
where
    WebSocketStream<S>: Stream,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (
        sink,
        stream
    ) = stream.split();

    (GraphQLSender { sink }, GraphQLReceiver { stream })
}
