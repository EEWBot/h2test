use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AHResult;
use chrono::{Local, DateTime};
use clap::{Parser, Subcommand};
use hickory_resolver::Resolver;
use hickory_resolver::config::*;
use rand::seq::IndexedRandom;
use tokio::net::TcpStream;
use tokio::sync::{watch, Semaphore};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{pki_types::ServerName, RootCertStore};

use hickory_resolver::name_server::TokioConnectionProvider;

const ALPN_H2: &str = "h2";
const HTTP2_SETTINGS_MAX_CONCURRENT_STREAMS: usize = 100;

#[derive(Debug, Parser)]
struct Cli {
    #[clap(subcommand)]
    subcommand: SubCommands,
}

#[derive(Debug, Subcommand)]
enum SubCommands {
    ManyRequest {
        #[clap(long)]
        interval: humantime::Duration,
    },

    PingPong {
        #[clap(long)]
        interval: humantime::Duration,
    }
}

async fn query_discord_ips() -> Vec<Ipv4Addr> {
    let resolver = Resolver::builder_with_config(
        ResolverConfig::default(),
        TokioConnectionProvider::default()
    ).build();

    let mut ips = vec![];
    let response = resolver.lookup_ip("discord.com").await.unwrap();
    ips.extend(response.iter().filter_map(|ip| {
        match ip {
            IpAddr::V4(ip) => Some(ip),
            _ => panic!("WTF!? discord.com provides IPv6 Addr"),
        }
    }));

    tracing::info!("I got {} ips in discord.com! {ips:?}", ips.len());

    ips
}

async fn many_request_sender(addr: &Ipv4Addr, sleep: Duration) -> AHResult<()> {
    let tls_client_config = Arc::new({
        let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut c = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        c.alpn_protocols.push(ALPN_H2.as_bytes().to_owned());

        c
    });

    let connector = TlsConnector::from(tls_client_config);

    let tcp = TcpStream::connect(format!("{}:443", addr)).await.unwrap();
    let dns_name = ServerName::try_from("discord.com").unwrap();
    let tls = connector.connect(dns_name, tcp).await?;

    {
        let (_, session) = tls.get_ref();

        let negotiated = session.alpn_protocol();
        let reference = Some(ALPN_H2.as_bytes());

        anyhow::ensure!(negotiated == reference, "Negotiated protocol is not HTTP/2");
    }

    let (mut client, connection) = h2::client::handshake(tls).await?;

    let (tx, mut rx) = watch::channel(None);

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tx.send(Some(e)).unwrap();
        };
    });

    // A value that will never violate the max parallel streams value.
    let semaphroe = Arc::new(Semaphore::new(HTTP2_SETTINGS_MAX_CONCURRENT_STREAMS / 2));

    tracing::info!("Connection established!");

    let local_date: DateTime<Local> = Local::now();
    let mut rng = rand::rng();
    let mut request_no = 0;

    let targets: Vec<http::Uri> = 
        include_str!("../targets.txt").split('\n').filter_map(|v| v.parse().ok())
        .collect();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(sleep) => {
                let mut headers = http::header::HeaderMap::new();
                headers.insert(http::header::CONTENT_TYPE, "application/json".parse().unwrap());
                headers.insert(http::header::USER_AGENT, "H2Tester/0.1.0".parse().unwrap());
                headers.insert(http::header::HOST, "discord.com".parse().unwrap());

                let method = http::method::Method::POST;
                let body = bytes::Bytes::from(format!("{{\"content\": \"Hello World ({local_date} / {request_no})\"}}"));

                tracing::info!("Request {request_no}");
                request_no += 1;

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
}

async fn pingpong_sender(addr: &Ipv4Addr, sleep: Duration) -> AHResult<()> {
    let tls_client_config = Arc::new({
        let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut c = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        c.alpn_protocols.push(ALPN_H2.as_bytes().to_owned());

        c
    });

    let connector = TlsConnector::from(tls_client_config);

    let tcp = TcpStream::connect(format!("{}:443", addr)).await.unwrap();
    let dns_name = ServerName::try_from("discord.com").unwrap();
    let tls = connector.connect(dns_name, tcp).await?;

    {
        let (_, session) = tls.get_ref();

        let negotiated = session.alpn_protocol();
        let reference = Some(ALPN_H2.as_bytes());

        anyhow::ensure!(negotiated == reference, "Negotiated protocol is not HTTP/2");
    }

    let (_client, mut connection) = h2::client::handshake(tls).await?;

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

    loop {
        tokio::select! {
            _ = tokio::time::sleep(sleep) => {
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

#[tokio::main]
async fn main() {
    let format = tracing_subscriber::fmt::format()
        .with_level(true)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(true)
        .compact();

    tracing_subscriber::fmt().event_format(format).with_max_level(tracing::Level::INFO).init();

    let cli = Cli::parse();

    match cli.subcommand {
        SubCommands::ManyRequest { interval } => {
            many_request_sender(&query_discord_ips().await[0], interval.into()).await.expect("Conn failed");
        }
        SubCommands::PingPong { interval } => {
            pingpong_sender(&query_discord_ips().await[0], interval.into()).await.expect("Conn failed");
        }
    }

}
