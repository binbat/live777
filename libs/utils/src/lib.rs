use std::env;
use tracing_subscriber::EnvFilter;

pub fn set_log(env_filter: String) {
    let _ = env::var("RUST_LOG").is_err_and(|_| {
        env::set_var("RUST_LOG", env_filter);
        true
    });
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .init();
}
