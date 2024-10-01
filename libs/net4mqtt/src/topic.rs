/// Publish:
/// TOPIC: <prefix>/<agent id>/<local id>/<label>/<protocol>/<address:port>
///
/// Pub topic example: prefix/agent-0/local-0/i/udp/127.0.0.1:4433
/// Pub topic example: prefix/agent-0/local-0/o/udp/127.0.0.1:4433
///
/// Subscribe:
/// TOPIC: <prefix>/< + | agent id>/< + | local id>/<label>/#
///
/// Sub topic example: prefix/+/local-0/i/#
/// Sub topic example: prefix/agent-0/+/o/#
///
/// About MQTT online status (Option)
///
/// TOPIC: prefix/agent-0/local-0/x/-/-
/// Retain: true

pub const ANY: &str = "+";
pub const ALL: &str = "#";
pub const NIL: &str = "-";

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
    agent_id: &str,
    local_id: &str,
    label: &str,
    protocol: &str,
    address: &str,
) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}",
        prefix, agent_id, local_id, label, protocol, address
    )
}

pub fn build_sub(prefix: &str, agent_id: &str, local_id: &str, label: &str) -> String {
    format!("{}/{}/{}/{}/{}", prefix, agent_id, local_id, label, ALL)
}

pub fn build_pub_x(prefix: &str, agent_id: &str, local_id: &str, label: &str) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}",
        prefix, agent_id, local_id, label, NIL, NIL
    )
}

pub fn parse(topic: &str) -> (&str, &str, &str, &str, &str, &str) {
    let v: Vec<&str> = topic.split(SPLIT).collect();
    (v[0], v[1], v[2], v[3], v[4], v[5])
}

#[test]
fn test_build_parse() {
    let prefix = "test_build_parse";
    let agent_id = "3";
    let local_id = "7";
    let address = "address";

    let result = "test_build_parse/3/7/i/kcp/address";

    assert_eq!(
        build(prefix, agent_id, local_id, label::I, protocol::KCP, address),
        result,
    );

    assert_eq!(
        build_sub(prefix, agent_id, local_id, label::I),
        "test_build_parse/3/7/i/#",
    );

    let (prefix2, agent_id2, local_id2, label2, protocol2, address2) = parse(result);
    assert_eq!(prefix2, prefix);
    assert_eq!(agent_id2, agent_id);
    assert_eq!(local_id2, local_id);
    assert_eq!(label2, label::I);
    assert_eq!(protocol2, protocol::KCP);
    assert_eq!(address2, address);

    assert_eq!(
        build_pub_x(prefix, agent_id, local_id, label::X),
        "test_build_parse/3/7/x/-/-",
    );
}
