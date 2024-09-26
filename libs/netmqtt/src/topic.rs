/// Publish:
/// TOPIC: <prefix>/<server id>/<client id>/<label>/<protocol>/<address:port>
///
/// Pub topic example: prefix/server-0/client-0/i/udp/127.0.0.1:4433
/// Pub topic example: prefix/server-0/client-0/o/udp/127.0.0.1:4433
///
/// Subscribe:
/// TOPIC: <prefix>/< + | server id>/< + | client id>/<label>/#
///
/// Sub topic example: prefix/+/client-0/i/#
/// Sub topic example: prefix/server-0/+/o/#
pub const ANY: &str = "+";
pub const ALL: &str = "#";

pub const NOSET: &str = "-";
const SPLIT: char = '/';

pub mod label {
    pub const I: &str = "i";
    pub const O: &str = "o";
    pub const X: &str = "x";
}

pub mod protocol {
    pub const KCP: &str = "kcp";
    pub const TCP: &str = "tcp";
    pub const UDP: &str = "udp";
}

pub fn build(
    prefix: &str,
    server_id: &str,
    client_id: &str,
    label: &str,
    protocol: &str,
    address: &str,
) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}",
        prefix, server_id, client_id, label, protocol, address
    )
}

pub fn build_sub(prefix: &str, server_id: &str, client_id: &str, label: &str) -> String {
    format!("{}/{}/{}/{}/{}", prefix, server_id, client_id, label, ALL)
}

pub fn build_pub_x(prefix: &str, server_id: &str, client_id: &str, label: &str) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}",
        prefix, server_id, client_id, label, NOSET, NOSET
    )
}

pub fn parse(topic: &str) -> (&str, &str, &str, &str, &str, &str) {
    let v: Vec<&str> = topic.split(SPLIT).collect();
    (v[0], v[1], v[2], v[3], v[4], v[5])
}

#[test]
fn test_build_parse() {
    let prefix = "test_build_parse";
    let server_id = "3";
    let client_id = "7";
    let address = "address";

    let result = "test_build_parse/3/7/i/kcp/address";

    assert_eq!(
        build(
            prefix,
            server_id,
            client_id,
            label::I,
            protocol::KCP,
            address
        ),
        result,
    );

    assert_eq!(
        build_sub(prefix, server_id, client_id, label::I),
        "test_build_parse/3/7/i/#",
    );

    let (prefix2, server_id2, client_id2, label2, protocol2, address2) = parse(result);
    assert_eq!(prefix2, prefix);
    assert_eq!(server_id2, server_id);
    assert_eq!(client_id2, client_id);
    assert_eq!(label2, label::I);
    assert_eq!(protocol2, protocol::KCP);
    assert_eq!(address2, address);

    assert_eq!(
        build_pub_x(prefix, server_id, client_id, label::X),
        "test_build_parse/3/7/x/-/-",
    );
}
