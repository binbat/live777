# Getting Started

## Minimize

### Run Live777 using docker:

::: danger
**You must use network host mode**
:::

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

### Install Live777

Download Binary from GitHub

```bash
./live777
```

### Configuration

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```


