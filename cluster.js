import { execFile, execSync } from 'node:child_process'
import { writeFile, rm } from 'node:fs/promises'
import { setTimeout } from 'timers/promises'
import { exit } from 'node:process'

async function reforward(server, streamId, url) {
    return fetch(`${server}/admin/reforward/${streamId}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
        },
        body: JSON.stringify({
            targetUrl: url,
        }),
    })
}

function buildAll() {
    execSync("npm run build")
    execSync("cargo build --release")
}

const appRust = "target/release/"
const appGo   = "gateway/"

const appGateway  = appGo   + "gateway"
const appLive777  = appRust + "live777"
const appWhipinto = appRust + "whipinto"
const appWhepfrom = appRust + "whepfrom"

const tmpCoreConfig = "cluster-test-config.toml"
//const tmpGateConfig = "cluster-gate-config.toml"
const tmpFFplaySdp  = "cluster-stream.sdp"

async function cluster() {
    const content = `
[cluster.storage]
model = "RedisStandalone"
addr = "redis://127.0.0.1:6379"
`;

    try {
       await writeFile(tmpCoreConfig, content);
    } catch (err) {
        console.error(err);
    }

    execSync("docker run -d --name redis --rm -p 6379:6379 redis")

    const live777EdgePort = "8888"
    const live777EdgeHost = `http://localhost:${live777EdgePort}`
    const live777EdgeStream = "888"
    const live777Edge = execFile(appLive777, ["--config", tmpCoreConfig], {
        env: { PORT: live777EdgePort },
    }, e => console.log(e))

    const live777CloudPort = "9999"
    const live777CloudHost = `http://localhost:${live777CloudPort}`
    const live777CloudStream = "999"
    const live777Cloud = execFile(appLive777, ["--config", tmpCoreConfig], {
        env: { PORT: live777CloudPort },
    }, e => console.log(e))

    const live777Gateway = execFile(appGateway)

    // === ===

    const whipinto = execFile(appWhipinto, [
        "-c", "vp8",
        "-u", `${live777EdgeHost}/whip/${live777EdgeStream}`,
        "--command",
        "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp 'rtp://127.0.0.1:{port}?pkt_size=1200'"
    ], e => console.log(e))

    await setTimeout(5000);

    const res = await reforward(live777EdgeHost, live777EdgeStream, `${live777CloudHost}/whip/${live777CloudStream}`)
    console.log(res.status === 200 ? "reforward success" : res.status)

    const whepfromPort = "5004"
    await writeFile(tmpFFplaySdp, `
v=0
m=video ${whepfromPort} RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
`);

    await setTimeout(5000);
    const whepfrom = execFile(appWhepfrom, [
        "-c", "vp8",
        "-u", `${live777CloudHost}/whep/${live777CloudStream}`,
        "-t", `127.0.0.1:${whepfromPort}`,
        "--command", `ffplay -protocol_whitelist rtp,file,udp -i ${tmpFFplaySdp}`
    ], e => console.log(e))

    await setTimeout(30000);


    await rm(tmpCoreConfig)
    await rm(tmpFFplaySdp)

    whepfrom.kill()
    whipinto.kill()

    // === ===

    live777Edge.kill()
    live777Cloud.kill()
    live777Gateway.kill()
    execSync("docker stop redis")
    console.log("=== All Done! ===")
}

await cluster()
exit()

