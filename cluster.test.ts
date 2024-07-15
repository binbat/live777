/* eslint  @stylistic/js/semi: ["error", "never"] */
/* eslint  @stylistic/js/quotes: ["error", "double"] */
/* eslint-disable @typescript-eslint/no-unused-expressions */

import { writeFile, rm } from "node:fs/promises"
import cp, { ChildProcess } from "node:child_process"

import { describe, beforeAll, afterAll, test, expect } from "vitest"

import { Stream } from "./web/shared/api"

async function sleep(ms: number): Promise<void> {
    return new Promise(resolve => {
        setTimeout(resolve, ms)
    })
}

interface SpawnOptions {
    env?: Record<string, string>;
    stdin?: null;
    stdout?: null;
    stderr?: null;
    onExit?: (e: ChildProcess) => void;
}

function toNodeSpawnOptions(o: SpawnOptions = {}): cp.SpawnOptions {
    return {
        env: o.env,
        stdio: [o.stdin ?? "pipe", o.stdout ?? "pipe", o.stderr ?? "pipe"]
    }
}

function spawn(command: string | string[], options?: SpawnOptions) {
    const cmd = Array.isArray(command) ? command[0] : command
    const arg = Array.isArray(command) ? command.slice(1) : []
    const process = cp.spawn(cmd, arg, toNodeSpawnOptions(options))
    if (options?.onExit) {
        process.on("exit", () => {
            options?.onExit?.(process)
        })
    }
    return process
}

function spawnSync(command: string | string[]) {
    const cmd = Array.isArray(command) ? command[0] : command
    const arg = Array.isArray(command) ? command.slice(1) : []
    return cp.spawnSync(cmd, arg)
}

async function getStreams(server: string): Promise<Stream[]> {
    return (await fetch(`${server}/api/streams/`)).json()
}

async function reforward(server: string, streamId: string, url: string) {
    return fetch(`${server}/api/cascade/${streamId}`, {
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
    srvs: ChildProcess[]
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

    const appLiveman = appRust + "liveman"
    const appLive777 = appRust + "live777"
    const appWhipinto = appRust + "whipinto"
    const appWhepfrom = appRust + "whepfrom"

    const tmpFileConfigVerge = "test_config-verge.toml"
    const tmpFileConfigCloud = "test_config-cloud.toml"
    const tmpFileConfigMan = "test_config-man.toml"
    const tmpFileFFplaySdp = "test_stream.sdp"

    const live777VergePort = "7778"
    const live777VergeHost = `http://${localhost}:${live777VergePort}`
    const live777VergeStream = "888"

    const live777CloudPort = "7779"
    const live777CloudHost = `http://${localhost}:${live777CloudPort}`
    const live777CloudStream = "999"

    const live777LivemanPort = "8080"
    const live777LivemanHost = `http://${localhost}:${live777LivemanPort}`
    const live777LivemanStream = "888"

    let serv: UpCluster

    beforeAll(async () => {
        const fileContentVerge = `
[strategy]
reforward_close_sub = true
`

        const fileContentCloud = `
[strategy]
reforward_close_sub = true
`
        const fileContentMan = `
[[nodes]]
alias = "test-verge"
url = "http://${localhost}:${live777VergePort}"
sub_max = 1

[[nodes]]
alias = "test-cloud"
url = "http://${localhost}:${live777CloudPort}"
`

        try {
            await writeFile(tmpFileConfigVerge, fileContentVerge)
            await writeFile(tmpFileConfigCloud, fileContentCloud)
            await writeFile(tmpFileConfigMan, fileContentMan)
        } catch (err) {
            console.error(err)
        }

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
                name: "live777Liveman",
                cmd: [appLiveman, "--config", tmpFileConfigMan],
                env: { PORT: live777LivemanPort },
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
            "--url", `${live777LivemanHost}/whep/${live777LivemanStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log("whepfrom 1", e.stderr) } })

        await sleep(1000)

        const whepfrom2 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777LivemanHost}/whep/${live777LivemanStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log("whepfrom 2", e.stderr) } })

        await sleep(1000)

        const res1 = (await getStreams(live777VergeHost)).find(r => r.id === live777LivemanStream)
        expect(res1).toBeTruthy()
        expect(res1?.subscribe.sessions.length).toEqual(1)
        const res2 = (await getStreams(live777CloudHost)).find(r => r.id === live777LivemanStream)
        expect(res2).toBeTruthy()
        expect(res2?.subscribe.sessions.length).toEqual(1)

        ffmpeg.kill(9)
        whepfrom1.kill()
        whepfrom2.kill()
        whipinto.kill()
    })

    afterAll(async () => {
        await rm(tmpFileConfigVerge)
        await rm(tmpFileConfigCloud)
        await rm(tmpFileConfigMan)
        serv?.down()

        console.log("=== All Done! ===")
    })
})
