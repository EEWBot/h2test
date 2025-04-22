use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use anyhow::Result as AHResult;
use bytes::Bytes;
use clap::{Parser, Subcommand};
use h2::client::{Connection, SendRequest};
use hickory_resolver::{config::ResolverConfig, name_server::TokioConnectionProvider, Resolver};
use rand::seq::IndexedRandom;
use tokio::net::TcpStream;
use tokio_rustls::{
    client::TlsStream,
    rustls::{pki_types::ServerName, RootCertStore},
    TlsConnector,
};

mod manyframes;
mod manyrequests;
mod pingpong;

pub const ALPN_H2: &str = "h2";
pub const HTTP2_SETTINGS_MAX_CONCURRENT_STREAMS: usize = 100;

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
    },

    ManyFrames {
        #[clap(long)]
        req_interval: humantime::Duration,

        #[clap(long)]
        ping_interval: humantime::Duration,

        #[clap(long)]
        req_count: usize,
    },
}

async fn query_discord_ips() -> Vec<Ipv4Addr> {
    let resolver = Resolver::builder_with_config(
        ResolverConfig::default(),
        TokioConnectionProvider::default(),
    )
    .build();

    let mut ips = vec![];
    let response = resolver.lookup_ip("discord.com").await.unwrap();
    ips.extend(response.iter().map(|ip| match ip {
        IpAddr::V4(ip) => ip,
        _ => panic!("WTF!? discord.com provides IPv6 Addr"),
    }));

    tracing::info!("I got {} ips in discord.com! {ips:?}", ips.len());

    ips
}

pub async fn setup_connection() -> AHResult<(SendRequest<Bytes>, Connection<TlsStream<TcpStream>>)>
{
    let tls_client_config = Arc::new({
        let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut c = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        c.alpn_protocols.push(ALPN_H2.as_bytes().to_owned());

        c
    });

    let ips = query_discord_ips().await;
    let ip = ips.choose(&mut rand::rng()).unwrap();

    let tcp = TcpStream::connect(format!("{}:443", ip)).await.unwrap();
    let dns_name = ServerName::try_from("discord.com").unwrap();

    let tls = TlsConnector::from(tls_client_config)
        .connect(dns_name, tcp)
        .await?;

    {
        let (_, session) = tls.get_ref();

        let negotiated = session.alpn_protocol();
        let reference = Some(ALPN_H2.as_bytes());

        anyhow::ensure!(negotiated == reference, "Negotiated protocol is not HTTP/2");
    }

    Ok(h2::client::handshake(tls).await?)
}

#[tokio::main]
async fn main() {
    let format = tracing_subscriber::fmt::format()
        .with_level(true)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(true)
        .compact();

    tracing_subscriber::fmt()
        .event_format(format)
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.subcommand {
        SubCommands::ManyRequest { interval } => {
            manyrequests::sender(interval.into())
                .await
                .expect("Conn failed");
        }
        SubCommands::PingPong { interval } => {
            pingpong::sender(interval.into())
                .await
                .expect("Conn failed");
        }
        SubCommands::ManyFrames {
            req_interval,
            ping_interval,
            req_count,
        } => {
            manyframes::sender(req_interval.into(), ping_interval.into(), req_count)
                .await
                .expect("Conn failed");
        }
    }
}
