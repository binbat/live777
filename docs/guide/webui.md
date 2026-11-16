# Web UI

This Web UI is build in live777

## Web WHIP/WHEP client

**Open your browser, enter the URL: `http://localhost:7777/`**

```
http://localhost:7777/
```

## Debugger

example:

```
http://localhost:7777/tools/debugger.html
```

You can use this test WebRTC-SVC

Both the WHIP publisher and the WHEP subscriber include an audio monitor
panel: live level meter, frequency spectrum, and per-track state
(`receiving` / `no RTP flow`), so you can tell whether audio is actually
flowing. The preview stays muted by default; untick `Mute` to hear it.
Right-click the video for extended audio stats (jitter, packet loss,
concealed samples).

## Single Page Player

example:

```
http://localhost:7777/tools/player.html?id=web-0&autoplay&controls&muted&reconnect=3000
```

URL params:

- `id`: string, live777 Stream ID
- `autoplay`: boolean
- `controls`: boolean
- `muted`: boolean, whether to mute by default
- `reconnect`: number, reconnect timeout in millisecond

