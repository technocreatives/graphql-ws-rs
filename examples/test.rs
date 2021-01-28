use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use graphql_ws::{raw::ClientPayload, GraphQLWebSocket, Request};

// https://github.com/serde-rs/serde/issues/994
mod json_string {
    use serde::de::{self, Deserialize, DeserializeOwned, Deserializer};
    use serde_json;

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: DeserializeOwned,
        D: Deserializer<'de>,
    {
        let j = String::deserialize(deserializer)?;
        serde_json::from_str(&j).map_err(de::Error::custom)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceMessage {
    characteristic_id: u16,
    #[serde(with = "json_string")]
    payload: serde_json::Value,
    timestamp: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubResponse {
    device_messages: DeviceMessage,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let req = Request::builder()
        .uri("ws://localhost:8000/subscriptions")
        .header(
            "Sec-WebSocket-Protocol",
            "elevate-bearer_eb64a6f0-9656-404d-a4c6-2e1fd1885e97,graphql-ws",
        )
        .body(())
        .unwrap();
    let mut socket = GraphQLWebSocket::connect(req).await.unwrap();

    let sub = socket.subscribe::<SubResponse>(ClientPayload {
        query: "subscription { deviceMessages(deviceId:\"b7c15f4d-1a20-4ee6-a4d4-f28c425f8b9c\") { characteristicId payload timestamp } }".into(),
        variables: None,
        operation_name: None,
    });

    let mut stream = sub.stream();

    while let Some(msg) = stream.next().await {
        match msg {
            Ok(payload) => println!("{:#?}", payload.data.unwrap().device_messages),
            Err(err) => println!("Error: {:?}", err),
        }
    }
}
