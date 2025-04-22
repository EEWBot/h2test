use std::time::Duration;

use anyhow::Result as AHResult;
use chrono::{DateTime, Local};
use tokio::sync::watch;

pub async fn sender(interval: Duration) -> AHResult<()> {
    let (_client, mut connection) = crate::setup_connection()
        .await
        .expect("Failed to connect to discord.com");

    let mut ping_pong = connection.ping_pong().unwrap();

    let (tx, mut rx) = watch::channel(None);

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tx.send(Some(e)).unwrap();
        };
    });

    tracing::info!("Connection established!");

    let mut counter = 0;
    let local_date: DateTime<Local> = Local::now();

    let mut interval = tokio::time::interval(interval);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                counter += 1;
                tracing::info!("{local_date} Send {counter}");
                let ping = h2::Ping::opaque();
                ping_pong.ping(ping).await?;
                tracing::info!("{local_date} Recv {counter}");
            },
            err = rx.changed() => err?,
        }
    }
}
