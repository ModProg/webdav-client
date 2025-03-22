//! Example with [`ureq`], currently fails because of <https://github.com/algesten/ureq/issues/1034>.
use std::env;

use webdav_client::{Auth, Depth};

fn main() {
    let client = ureq::agent();

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
        .unwrap();

    eprintln!("{result:#?}");
}
