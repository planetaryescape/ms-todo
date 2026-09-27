//! Where an attachment's download may go (D-067): the URL someone typed
//! may name any host, but no redirect may lead to this machine or its
//! network (loopback, private, link-local, unique-local and the like),
//! where a server could otherwise have the daemon fetch what only it can
//! reach, the typed host included (another port, a later DNS answer). The
//! check is in the resolver the connection itself uses, so a name can't
//! resolve to one address when checked and another when connected; an
//! address written in the URL is checked before the request, since no
//! resolver sees it. The first request and the redirects use separate
//! clients, so no redirect reuses the first request's connection.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Resolves only to public addresses, or, for the first request's client
/// (`trusted`), to anything.
pub(crate) struct PublicOnly {
    pub trusted: bool,
}

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_ascii_lowercase();
        let trusted = self.trusted;
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if !trusted && let Some(private) = found.iter().find(|addr| !is_public(addr.ip())) {
                return Err(format!(
                    "{host} is at {}, on this machine or its network, which a redirect may not \
                     lead to",
                    private.ip()
                )
                .into());
            }
            let addrs: Addrs = Box::new(found.into_iter());
            Ok(addrs)
        })
    }
}

/// Whether a redirect to `url` may be requested, as far as its host goes:
/// a name (the resolver checks it) or a public address.
pub(crate) fn redirect_allowed(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    if host.is_empty() {
        return false;
    }
    // An IPv6 host comes in brackets; a name isn't an address at all.
    match host.trim_start_matches('[').trim_end_matches(']').parse() {
        Ok(ip) => is_public(ip),
        Err(_) => true,
    }
}

/// Not this machine, its network, or an address no server has.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => public_v6(ip),
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        // Shared address space (carrier-grade NAT), 100.64.0.0/10.
        || (a == 100 && (64..128).contains(&b))
        // "This network", 0.0.0.0/8.
        || a == 0)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local, fc00::/7.
        || (first & 0xfe00) == 0xfc00
        // Link-local, fe80::/10.
        || (first & 0xffc0) == 0xfe80)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(ip: &str) -> bool {
        is_public(ip.parse().expect("ip"))
    }

    #[test]
    fn this_machine_and_its_network_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.5",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!public(ip), "{ip}");
        }
        for ip in [
            "93.184.216.34",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "100.128.0.1",
        ] {
            assert!(public(ip), "{ip}");
        }
    }

    #[test]
    fn a_redirect_may_not_lead_to_a_private_address() {
        let url = |text: &str| Url::parse(text).expect("url");
        assert!(redirect_allowed(&url("https://example.com/a")));
        assert!(!redirect_allowed(&url("https://169.254.169.254/x")));
        assert!(!redirect_allowed(&url("https://[::1]/x")));
        assert!(
            !redirect_allowed(&url("https://192.168.1.5/b")),
            "typed or not"
        );
        assert!(redirect_allowed(&url("https://1.1.1.1/x")));
    }

    #[tokio::test]
    async fn a_name_on_this_machine_is_refused_but_for_the_first_request() {
        let name = |host: &str| host.parse::<Name>().expect("name");
        let strict = PublicOnly { trusted: false };
        assert!(strict.resolve(name("localhost")).await.is_err());
        let first = PublicOnly { trusted: true };
        assert!(first.resolve(name("localhost")).await.is_ok());
    }
}
