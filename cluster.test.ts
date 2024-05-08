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

interface ServerOptions {
    name: string;
    cmd: string[];
    env: Record<string, string>;
}

class UpCluster {
    srvs: any[]
    constructor(servers: ServerOptions[]) {
        this.srvs = servers.map(s => spawn(s.cmd, {
            env: s.env,
            onExit: e => {
                e.exitCode && console.log(`${s.name}: ${e.stderr}`)
                this.down()
            },
        }))
    }

    down() {
        this.srvs.forEach(s => s.kill())
    }
}

describe("test cluster", () => {
    const localhost = "127.0.0.1"

    const appRust = "target/release/"

    const appGateway  = appRust + "live777-gateway"
    const appLive777  = appRust + "live777"
    const appWhipinto = appRust + "whipinto"
    const appWhepfrom = appRust + "whepfrom"

    const tmpFileConfigVerge  = "test_config-edge.toml"
    const tmpFileConfigCloud = "test_config-cloud.toml"
    const tmpFileConfigGate  = "test_config-gate.toml"
    const tmpFileFFplaySdp   = "test_stream.sdp"

    const live777VergePort = "7778"
    const live777VergeHost = `http://${localhost}:${live777VergePort}`
    const live777VergeStream = "888"

    const live777CloudPort = "7779"
    const live777CloudHost = `http://${localhost}:${live777CloudPort}`
    const live777CloudStream = "999"

    const live777GatewayPort = "8080"
    const live777GatewayHost = `http://${localhost}:${live777GatewayPort}`
    const live777GatewayStream = "888"

    let serv: UpCluster

    beforeAll(async () => {
        const fileContentVerge = `
[node_info]
ip_port = "${localhost}:${live777VergePort}"

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
     const fileContentGate = `
[node_info.storage]
model = "RedisStandalone"
addr = "redis://127.0.0.1:6379"
`

        try {
            await writeFile(tmpFileConfigVerge, fileContentVerge)
            await writeFile(tmpFileConfigCloud, fileContentCloud)
            await writeFile(tmpFileConfigGate, fileContentGate)
        } catch (err) {
            console.error(err)
        }

        console.log(spawnSync(["docker", "run", "-d", "--name", "redis", "--rm", "-p", "6379:6379", "redis"]).stderr.toString())

        serv = new UpCluster([
            {
                name: "live777Verge",
                cmd: [appLive777, "--config", tmpFileConfigVerge],
                env: { PORT: live777VergePort },
            }, {
                name: "live777Cloud",
                cmd: [appLive777, "--config", tmpFileConfigCloud],
                env: { PORT: live777CloudPort },
            }, {
                name: "live777Gateway",
                cmd: [appGateway, "--config", tmpFileConfigGate],
                env: { PORT: live777GatewayPort },
            }
        ])
    })

    test("reforward", async () => {
        const whipintoPort = "5003"
        const whipinto = spawn([
            appWhipinto,
            "--codec", "vp8",
            "--url", `${live777VergeHost}/whip/${live777VergeStream}`,
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
            const res = await reforward(`http://127.0.0.1:${live777VergePort}`, live777VergeStream, `${live777CloudHost}/whip/${live777CloudStream}`)
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
            "--command", `ffplay -nodisp -protocol_whitelist rtp,file,udp -i ${tmpFileFFplaySdp}`
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
        const whipintoPort = "5005"
        const whipinto = spawn([
            appWhipinto,
            "--codec", "vp8",
            "--url", `${live777VergeHost}/whip/${live777VergeStream}`,
            "--port", whipintoPort,
        ], { onExit: e => { e.exitCode && console.log("whipinto", e.stderr) } })

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

        const whepfromPort = "5006"
        const whepfrom1 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777GatewayHost}/whep/${live777GatewayStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log("whepfrom 1", e.stderr) } })

        await sleep(1000)

        const whepfrom2 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777GatewayHost}/whep/${live777GatewayStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log("whepfrom 2", e.stderr) } })

        await sleep(1000)

        const res1 = (await info(live777VergeHost)).find(r => r.id === live777GatewayStream)
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
        await rm(tmpFileConfigVerge)
        await rm(tmpFileConfigCloud)
        await rm(tmpFileConfigGate)
        serv.down()

        console.log(spawnSync(["docker", "stop", "redis"]).stderr.toString())
        console.log("=== All Done! ===")
    })
})
