FROM rust:slim-bookworm as builder

WORKDIR /app

COPY . .

RUN cargo build --release

FROM debian:bookworm-slim

COPY --from=builder /app/target/release/live777 /usr/local/bin/live777

CMD ["live777"]
