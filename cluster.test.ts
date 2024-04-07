import { describe, beforeAll, afterAll, test } from "bun:test";
import { spawn, spawnSync, sleep } from "bun";
import { writeFile, rm } from 'node:fs/promises'

async function reforward(server: string, streamId: string, url: string) {
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

describe("test cluster", () => {
    const appRust = "target/release/"
    const appGo   = "gateway/"

    const appGateway  = appGo   + "gateway"
    const appLive777  = appRust + "live777"
    const appWhipinto = appRust + "whipinto"
    const appWhepfrom = appRust + "whepfrom"

    const tmpCoreConfig = "cluster-test-config.toml"
    //const tmpGateConfig = "cluster-gate-config.toml"
    const tmpFFplaySdp  = "cluster-stream.sdp"


    const live777EdgePort = "8888"
    const live777EdgeHost = `http://localhost:${live777EdgePort}`
    const live777EdgeStream = "888"

    const live777CloudPort = "9999"
    const live777CloudHost = `http://localhost:${live777CloudPort}`
    const live777CloudStream = "999"

    let live777Edge: any
    let live777Cloud: any

    beforeAll(async () => {
        console.log("beforeAll")

        const content = `
[cluster]
max = "1"

[cluster.storage]
model = "RedisStandalone"
addr = "redis://127.0.0.1:6379"
`;

        try {
            await writeFile(tmpCoreConfig, content);
        } catch (err) {
            console.error(err);
        }

        console.log(spawnSync(["docker", "run", "-d", "--name", "redis", "--rm", "-p", "6379:6379", "redis"]).stderr.toString())

        live777Edge = spawn([appLive777, "--config", tmpCoreConfig], {
            env: { PORT: live777EdgePort },
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        live777Cloud = spawn([appLive777, "--config", tmpCoreConfig], {
            env: { PORT: live777CloudPort },
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        //live777Gateway = spawn([appGateway])
    })


    test("reforward", async () => {
        const whipintoPort = "5003"
        const whipinto = spawn([
            appWhipinto,
            "--codec", "vp8",
            "--url", `${live777EdgeHost}/whip/${live777EdgeStream}`,
            "--port", whipintoPort,
        ], { onExit: e => { e.exitCode && console.log(e.stderr) } })

        const ffmpeg = spawn([
            "ffmpeg", "-re", "-f", "lavfi",
            "-i", "testsrc=size=640x480:rate=30",
            "-vcodec", "libvpx",
            "-cpu-used", "5", "-deadline", "1",
            "-g", "10", "-error-resilient", "1",
            "-auto-alt-ref", "1", "-f", "rtp", `rtp://127.0.0.1:${whipintoPort}?pkt_size=1200`
        ], { onExit: e => { e.exitCode && console.log(e.stderr) } })

        await sleep(1000)

        try {
            const res = await reforward(`http://127.0.0.1:${live777EdgePort}`, live777EdgeStream, `${live777CloudHost}/whip/${live777CloudStream}`)
            console.log(res.status === 200 ? "reforward success" : res.status)
        } catch (e) {
            console.log(e)
        }

        const whepfromPort = "5004"
        await writeFile(tmpFFplaySdp, `
v=0
m=video ${whepfromPort} RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
`)

        await sleep(1000)
        const whepfrom = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777CloudHost}/whep/${live777CloudStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
            "--command", `ffplay -protocol_whitelist rtp,file,udp -i ${tmpFFplaySdp}`
        ], {
                stdout: null,
                onExit: e => { e.exitCode && console.log(e.stderr) },
            })

        await sleep(2000)
        await rm(tmpFFplaySdp)

        ffmpeg.kill()
        whepfrom.kill()
        whipinto.kill()
    })

    afterAll(async () => {
        console.log("afterAll")
        await rm(tmpCoreConfig)
        live777Edge.kill()
        live777Cloud.kill()
        //live777Gateway.kill()

        console.log(spawnSync(["docker", "stop", "redis"]).stderr.toString())
        console.log("=== All Done! ===")
    })
})
