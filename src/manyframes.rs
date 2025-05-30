use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AHResult;
use chrono::{DateTime, Local};
use http::header::{HeaderMap, CONTENT_TYPE, HOST, USER_AGENT};
use rand::seq::IndexedRandom;
use tokio::sync::{watch, Semaphore};

pub async fn sender(
    req_interval: Duration,
    ping_interval: Duration,
    req_count: usize,
) -> AHResult<()> {
    let (mut client, mut connection) = crate::setup_connection()
        .await
        .expect("Failed to connect to discord.com");

    let (tx, mut rx) = watch::channel(None);

    let mut ping_pong = connection.ping_pong().unwrap();

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tx.send(Some(e)).unwrap();
        };
    });

    // A value that will never violate the max parallel streams value.
    let semaphroe = Arc::new(Semaphore::new(
        crate::HTTP2_SETTINGS_MAX_CONCURRENT_STREAMS / 2,
    ));

    tracing::info!("Connection established!");

    let local_date: DateTime<Local> = Local::now();
    let mut rng = rand::rng();

    let targets: Vec<http::Uri> = include_str!("../targets.txt")
        .split('\n')
        .filter_map(|v| v.parse().ok())
        .collect();

    let mut interval = tokio::time::interval(req_interval);

    for request_no in 0..req_count{
        tokio::select! {
            _ = interval.tick() => {
                let mut headers = HeaderMap::new();
                headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
                headers.insert(USER_AGENT, "H2Tester/0.1.0".parse().unwrap());
                headers.insert(HOST, "discord.com".parse().unwrap());

                let method = http::method::Method::POST;
                let body = bytes::Bytes::from(format!("{{\"content\": \"Hello World ({local_date} / {request_no})\"}}"));

                tracing::info!("Request {request_no}");

                let permit = semaphroe.clone().acquire_owned().await.unwrap();
                let target = targets.choose(&mut rng).unwrap();

                let mut request = http::Request::builder().method(method).uri(target).body(()).unwrap();
                *request.headers_mut() = headers.clone();

                let (response, mut respond) = client.send_request(request, false)?;

                respond.reserve_capacity(body.len());
                respond.send_data(body.clone(), true)?;

                tokio::spawn(async move {
                    let status = response.await.unwrap().status();
                    drop(permit);

                    if status.is_success() {
                        tracing::info!("Code: {}, {request_no}", status);
                    } else {
                        tracing::error!("Code: {}, {request_no}", status);
                    }
                });
            },
            err = rx.changed() => err?,
        }
    }


    let mut interval = tokio::time::interval(ping_interval);

    let mut counter = 0;

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
