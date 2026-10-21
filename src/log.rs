pub fn set(env_filter: String) {
    set_with(env_filter, std::io::stdout);
}

#[allow(dead_code)]
pub fn set_stderr(env_filter: String) {
    set_with(env_filter, std::io::stderr);
}

fn set_with<W>(env_filter: String, writer: W)
where
    W: for<'writer> tracing_subscriber::fmt::writer::MakeWriter<'writer> + Send + Sync + 'static,
{
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new(env_filter)))
        .with_writer(writer)
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .init();
}
