//! Example with [`minreq`], here we are using the "marker client"
//! [`Minreq`](webdav_client::Minreq) as [`minreq`] it self doesn't have a
//! client.
use std::env;

use webdav_client::{Auth, Depth, Minreq};

fn main() {
    let auth = env::var("LOGIN")
        .map(|username| Auth::Basic {
            username,
            password: env::var("PASSWORD").ok(),
        })
        .unwrap_or(Auth::None);

    let result = webdav_client::Client::authenticated(Minreq, auth)
        .prop_find(
            env::var("HOST").unwrap(),
            Depth::Some(1),
            ["d:displayname"],
            [("d", "DAV:")],
        )
        .unwrap();

    eprintln!("{result:#?}");
}
