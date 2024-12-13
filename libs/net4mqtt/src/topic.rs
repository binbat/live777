//! Publish:
//! TOPIC: <prefix>/<agent id>/<local id>/<label>/<protocol>/<src(address:port)>/<dst(address:port)>
//!
//! Pub topic example: prefix/agent-0/local-0/i/udp/127.0.0.1:4444/127.0.0.1:4433
//! Pub topic example: prefix/agent-0/local-0/o/udp/127.0.0.1:4444/127.0.0.1:4433
//!
//! Subscribe:
//! TOPIC: <prefix>/< + | agent id>/< + | local id>/<label>/#
//!
//! Sub topic example: prefix/+/local-0/i/#
//! Sub topic example: prefix/agent-0/+/o/#
//!
//! About MQTT online status (Option)
//!
//! TOPIC: prefix/agent-0/local-0/v/-
//! Retain: true

pub const ANY: &str = "+";
pub const ALL: &str = "#";
pub const NIL: &str = "-";

const SPLIT: char = '/';

pub mod label {
    pub const I: &str = "i";
    pub const O: &str = "o";
    pub const V: &str = "v";
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
    src: &str,
    dst: &str,
) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}/{}",
        prefix, agent_id, local_id, label, protocol, src, dst
    )
}

pub fn build_sub(prefix: &str, agent_id: &str, local_id: &str, label: &str) -> String {
    format!("{}/{}/{}/{}/{}", prefix, agent_id, local_id, label, ALL)
}

pub fn build_pub_x(prefix: &str, agent_id: &str, local_id: &str, label: &str) -> String {
    format!("{}/{}/{}/{}/{}", prefix, agent_id, local_id, label, NIL)
}

pub fn parse(topic: &str) -> (&str, &str, &str, &str, &str, &str, &str) {
    let mut v: Vec<&str> = topic.split(SPLIT).collect();
    v.extend((0..(7 - v.len())).map(|_| NIL).collect::<Vec<&str>>());
    (v[0], v[1], v[2], v[3], v[4], v[5], v[6])
}

#[test]
fn test_build_parse() {
    let prefix = "test_build_parse";
    let agent_id = "3";
    let local_id = "7";
    let src = "src";
    let dst = "dst";

    let result = "test_build_parse/3/7/i/kcp/src/dst";

    assert_eq!(
        build(
            prefix,
            agent_id,
            local_id,
            label::I,
            protocol::KCP,
            src,
            dst
        ),
        result,
    );

    assert_eq!(
        build_sub(prefix, agent_id, local_id, label::I),
        "test_build_parse/3/7/i/#",
    );

    assert_eq!(
        (
            prefix,
            agent_id,
            local_id,
            label::I,
            protocol::KCP,
            src,
            dst
        ),
        parse(result)
    );

    assert_eq!(
        build_pub_x(prefix, agent_id, local_id, label::V),
        "test_build_parse/3/7/v/-",
    );

    assert_eq!(("a", "b", NIL, NIL, NIL, NIL, NIL), parse("a/b"));
}
