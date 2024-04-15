import { describe, beforeAll, afterAll, test, expect } from "bun:test"
import { spawn, spawnSync, sleep } from "bun"
import { writeFile, rm } from 'node:fs/promises'

async function info(server: string): Promise<any> {
    return await (await fetch(`${server}/admin/infos`)).json()
}

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
    const localhost = "127.0.0.1"

    //const appRust = "target/debug/"
    const appRust = "target/release/"
    const appGo   = "gateway/"

    const appGateway  = appGo   + "gateway"
    const appLive777  = appRust + "live777"
    const appWhipinto = appRust + "whipinto"
    const appWhepfrom = appRust + "whepfrom"

    const tmpFileConfigEdge  = "test_config-edge.toml"
    const tmpFileConfigCloud = "test_config-cloud.toml"
    //const tmpFileConfigGate  = "test_config-gate.toml"
    const tmpFileFFplaySdp   = "test_stream.sdp"

    const live777EdgePort = "7778"
    const live777EdgeHost = `http://${localhost}:${live777EdgePort}`
    const live777EdgeStream = "888"

    const live777CloudPort = "7779"
    const live777CloudHost = `http://${localhost}:${live777CloudPort}`
    const live777CloudStream = "999"

    const live777GatewayPort = "8080"
    const live777GatewayHost = `http://${localhost}:${live777GatewayPort}`
    const live777GatewayStream = "888"

    let live777Edge: any
    let live777Cloud: any
    let live777Gateway: any

    beforeAll(async () => {
        const fileContentEdge = `
[node_info]
ip_port = "${localhost}:${live777EdgePort}"

[node_info.storage]
model = "RedisStandalone"
addr = "redis://127.0.0.1:6379"

[node_info.meta_data]
pub_max = 1
sub_max = 1
reforward_cascade = false
reforward_close_sub = true
`

        const fileContentCloud = `
[node_info]
ip_port = "${localhost}:${live777CloudPort}"

[node_info.storage]
model = "RedisStandalone"
addr = "redis://127.0.0.1:6379"

[node_info.meta_data]
pub_max = 1
sub_max = 100
reforward_cascade = false
reforward_close_sub = true
`
        try {
            await writeFile(tmpFileConfigEdge, fileContentEdge)
            await writeFile(tmpFileConfigCloud, fileContentCloud)
        } catch (err) {
            console.error(err)
        }

        console.log(spawnSync(["docker", "run", "-d", "--name", "redis", "--rm", "-p", "6379:6379", "redis"]).stderr.toString())

        live777Edge = spawn([appLive777, "--config", tmpFileConfigEdge], {
            env: { PORT: live777EdgePort },
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        live777Cloud = spawn([appLive777, "--config", tmpFileConfigCloud], {
            env: { PORT: live777CloudPort },
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        live777Gateway = spawn([appGateway], {
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })
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
        ], {
                stderr: null,
                onExit: e => { e.exitCode && console.log(e.stderr) },
            })

        await sleep(1000)

        try {
            const res = await reforward(`http://127.0.0.1:${live777EdgePort}`, live777EdgeStream, `${live777CloudHost}/whip/${live777CloudStream}`)
            console.log(res.status === 200 ? "reforward success" : res.status)
        } catch (e) {
            console.log(e)
        }

        const whepfromPort = "5004"
        await writeFile(tmpFileFFplaySdp, `
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
            "--command", `ffplay -protocol_whitelist rtp,file,udp -i ${tmpFileFFplaySdp}`
        ], {
                stdout: null,
                stderr: null,
                onExit: e => { e.exitCode && console.log(e.stderr) },
            })

        await sleep(2000)
        await rm(tmpFileFFplaySdp)

        ffmpeg.kill(9)
        whepfrom.kill()
        whipinto.kill()
    })

    test("p2p to sfu", async () => {
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
        ], {
                stderr: null,
                onExit: e => { e.exitCode && console.log(e.stderr) },
            })

        await sleep(1000)

        const whepfromPort = "5004"
        const whepfrom1 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777GatewayHost}/whep/${live777GatewayStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log(e.stderr) } })

        await sleep(1000)

        const whepfrom2 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777GatewayHost}/whep/${live777GatewayStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log(e.stderr) } })

        await sleep(1000)

        const res1 = (await info(live777EdgeHost)).find(r => r.id === live777GatewayStream)
        expect(res1).toBeTruthy()
        expect(res1.subscribeSessionInfos.length).toEqual(1)
        const res2 = (await info(live777CloudHost)).find(r => r.id === live777GatewayStream)
        expect(res2).toBeTruthy()
        expect(res2.subscribeSessionInfos.length).toEqual(1)

        ffmpeg.kill(9)
        whepfrom1.kill()
        whepfrom2.kill()
        whipinto.kill()
    })


    afterAll(async () => {
        await rm(tmpFileConfigEdge)
        await rm(tmpFileConfigCloud)
        //await rm(tmpFileConfigGate)
        live777Edge.kill()
        live777Cloud.kill()
        live777Gateway.kill()

        console.log(spawnSync(["docker", "stop", "redis"]).stderr.toString())
        console.log("=== All Done! ===")
    })
})
