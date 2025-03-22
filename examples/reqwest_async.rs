//! Example with [`reqwest`] using the non-blocking [`Client`](reqwest::Client).
use std::env;

use webdav_client::{Auth, Depth};

#[tokio::main]
async fn main() {
    let client = reqwest::Client::new();

    let auth = env::var("LOGIN")
        .map(|username| Auth::Basic {
            username,
            password: env::var("PASSWORD").ok(),
        })
        .unwrap_or(Auth::None);

    let result = webdav_client::Client::authenticated(client, auth)
        .prop_find(
            env::var("HOST").unwrap(),
            Depth::Some(1),
            ["d:displayname"],
            [("d", "DAV:")],
        )
        .await
        .unwrap();

    eprintln!("{result:#?}");
}
