const SPLIT: char = '.';

pub struct Kxdns {
    domain: String,
}

impl Kxdns {
    pub fn new(domain: String) -> Self {
        Self { domain }
    }

    pub fn registry(&self, key: &str) -> String {
        format!("{}.{}", key, self.domain)
    }

    pub fn resolver(raw: &str) -> &str {
        let v: Vec<&str> = raw.split(SPLIT).collect();
        v[0]
    }
}

#[test]
fn test_kxdns() {
    let kxdns = Kxdns::new("kxdns.com".to_string());
    let key = "test";
    let domain = kxdns.registry(key);
    assert_eq!(domain, "test.kxdns.com");
    let key2 = Kxdns::resolver(&domain);
    assert_eq!(key, key2);
}
