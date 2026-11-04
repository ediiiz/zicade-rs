//! Which `Proxy-Authorization` keyword the proxy answers an upstream with:
//! the one the gateway's first 407 advertises (`NTLM` or `Negotiate`), kept
//! for the rest of that connection's handshake, and none at all when the
//! gateway offers nothing the proxy can answer.

mod common;

use common::TestProxy;
use common::client::{ReqBody, proxy_connect, proxy_http};
use common::scheme_fake::{
    SchemeScript, SchemeUpstream, challenge_checking_factory, expected_header,
    spawn_scheme_upstream,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zicade_proxy::{Routing, UpstreamTarget};

async fn proxy_for(upstream: &SchemeUpstream) -> TestProxy {
    TestProxy::spawn_with_routing(Routing::Upstream(UpstreamTarget {
        addr: upstream.addr.to_string(),
        auth: challenge_checking_factory(),
    }))
    .await
}

/// Open a CONNECT tunnel through `proxy` and prove bytes echo across it.
async fn assert_tunnel_echoes(proxy: &TestProxy) {
    let mut tunnel = proxy_connect(proxy.addr, "example.com:443").await;
    tunnel.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tunnel.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
}

fn both_legs(keyword: &str) -> Vec<String> {
    vec![expected_header(keyword, 1), expected_header(keyword, 2)]
}

#[tokio::test]
async fn ntlm_gateway_offering_basic_first_gets_ntlm_on_every_leg() {
    let upstream = spawn_scheme_upstream(SchemeScript {
        bare: vec!["Basic realm=\"x\"", "NTLM"],
        challenge: vec!["Basic realm=\"x\"", "NTLM {challenge}"],
        accept: Some("NTLM"),
    })
    .await;
    let proxy = proxy_for(&upstream).await;

    assert_tunnel_echoes(&proxy).await;
    assert_eq!(upstream.authorizations(), both_legs("NTLM"));

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn ntlm_gateway_offering_ntlm_first_gets_ntlm_on_every_leg() {
    let upstream = spawn_scheme_upstream(SchemeScript {
        bare: vec!["NTLM", "Basic realm=\"x\""],
        challenge: vec!["NTLM {challenge}", "Basic realm=\"x\""],
        accept: Some("NTLM"),
    })
    .await;
    let proxy = proxy_for(&upstream).await;

    assert_tunnel_echoes(&proxy).await;
    assert_eq!(upstream.authorizations(), both_legs("NTLM"));

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_forward_through_ntlm_gateway_uses_ntlm() {
    let upstream = spawn_scheme_upstream(SchemeScript {
        bare: vec!["Basic realm=\"x\"", "NTLM"],
        challenge: vec!["Basic realm=\"x\"", "NTLM {challenge}"],
        accept: Some("NTLM"),
    })
    .await;
    let proxy = proxy_for(&upstream).await;

    let body = b"forwarded".to_vec();
    let resp = proxy_http(
        proxy.addr,
        "POST",
        "http://example.com/echo",
        &[],
        ReqBody::ContentLength(body.clone()),
    )
    .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, body);
    assert_eq!(upstream.authorizations(), both_legs("NTLM"));

    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn negotiate_gateway_still_gets_negotiate() {
    let upstream = spawn_scheme_upstream(SchemeScript {
        bare: vec!["Negotiate", "Basic realm=\"x\""],
        challenge: vec!["Negotiate {challenge}"],
        accept: Some("Negotiate"),
    })
    .await;
    let proxy = proxy_for(&upstream).await;

    assert_tunnel_echoes(&proxy).await;
    assert_eq!(upstream.authorizations(), both_legs("Negotiate"));

    proxy.shutdown().await.unwrap();
}

/// A 407 that names no scheme at all keeps the RFC 4559 `Negotiate` keyword.
#[tokio::test]
async fn gateway_naming_no_scheme_gets_negotiate() {
    let upstream = spawn_scheme_upstream(SchemeScript {
        bare: vec![],
        challenge: vec!["Negotiate {challenge}"],
        accept: Some("Negotiate"),
    })
    .await;
    let proxy = proxy_for(&upstream).await;

    assert_tunnel_echoes(&proxy).await;
    assert_eq!(upstream.authorizations(), both_legs("Negotiate"));

    proxy.shutdown().await.unwrap();
}
