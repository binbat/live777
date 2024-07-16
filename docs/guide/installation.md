# Installation

## Download Binary from GitHub

you can donwload binary from [here](https://github.com/binbat/live777/releases)

```bash
./live777
```

### Configuration

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```

## Docker

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

## Cargo

```bash
cargo install --git http://github.com/binbat/live777 whipinto
cargo install --git http://github.com/binbat/live777 whepfrom
```

## Windows

**Winget**

```bash
winget install live777
winget install whipinto
winget install whepfrom
```

