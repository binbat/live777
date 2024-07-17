/* eslint @stylistic/js/semi: ["error", "never"] */
/* eslint @stylistic/js/quotes: ["error", "double"] */
/* eslint-disable @typescript-eslint/no-unused-expressions */

import { writeFile, rm } from "node:fs/promises"
import { text } from "node:stream/consumers"
import cp, { ChildProcess } from "node:child_process"

import { describe, beforeAll, afterAll, test, expect, assert, beforeEach, afterEach } from "vitest"

import { Stream } from "./web/shared/api"

async function sleep(ms: number): Promise<void> {
    return new Promise(resolve => {
        setTimeout(resolve, ms)
    })
}

interface ExitedProcess {
    exitCode: number;
    stderr: string;
}

interface SpawnOptions {
    env?: Record<string, string>;
    stdin?: null;
    stdout?: null;
    stderr?: null;
    onExit?: (e: ExitedProcess) => void;
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
    process.on("exit", async () => {
        const exitCode = process.exitCode ?? 0
        const stderr = (process.stderr && await text(process.stderr)) ?? ""
        options?.onExit?.({ exitCode, stderr })
    })
    return process
}

function spawnSync(command: string | string[]) {
    const cmd = Array.isArray(command) ? command[0] : command
    const arg = Array.isArray(command) ? command.slice(1) : []
    return cp.spawnSync(cmd, arg)
}

async function until<T>(fn: () => Promise<T>, predicate: (t: T) => boolean, interval = 100): Promise<void> {
    do {
        await sleep(interval)
    } while (!predicate(await fn()))
}

async function untilHttpOk(host: string): Promise<void> {
    await until(() => fetch(host).then(r => r.ok).catch(() => false), ok => ok)
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
            stderr: null,
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

describe("test single live777", () => {
    const localhost = "127.0.0.1"

    const appRust = "target/release/"
    const appLive777 = appRust + "live777"
    const live777Port = "7777"
    const live777Host = `http://${localhost}:${live777Port}`

    let live777: ChildProcess | null = null

    beforeEach(async () => {
        live777 = spawn([
            appLive777,
        ], { env: { PORT: live777Port }, onExit: e => { e.exitCode && console.log(e.stderr) } })
        await untilHttpOk(live777Host)
    })

    afterEach(async () => {
        live777?.kill()
    })

    test("minimum", async () => {
        try {
            const res = await getStreams(live777Host)
            expect(res).toHaveLength(0)
        } catch {
            assert.fail()
        }
    })

    test("create stream", async () => {
        const live777Stream = "888"
        try {
            const resCreate = await fetch(`${live777Host}/api/streams/${live777Stream}`, {
                method: "POST",
            })
            expect(resCreate.status).toBe(204)
            const streams = await getStreams(live777Host)
            expect(streams).toHaveLength(1)
        } catch (e) {
            assert.fail(e)
        }
    })

    test("create stream connect", async () => {
        const live777Stream = "888"
        const appWhipinto = appRust + "whipinto"

        try {
            const resCreate = await fetch(`${live777Host}/api/streams/${live777Stream}`, {
                method: "POST",
            })

            expect(resCreate.status).toBe(204)

            const whipinto = spawn([
                appWhipinto,
                "--codec", "vp8",
                "--url", `${live777Host}/whip/${live777Stream}`,
                "--port", "5003",
            ], { onExit: e => { e.exitCode && console.log(e.stderr) } })

            await until(() => getStreams(live777Host), s => s[0]?.publish.sessions.length > 0)

            try {
                const resIndex = (await getStreams(live777Host)).find(r => r.id === live777Stream)
                expect(resIndex?.publish.sessions).toHaveLength(1)
                expect(resIndex?.publish.sessions[0].state).toBe("connected")
            } catch (e) {
                assert.fail(e)
            } finally {
                whipinto.kill()
            }

        } catch (e) {
            assert.fail(e)
        }
    })
})

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

    let serv: UpCluster | null

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
[reforward]
close_other_sub = true

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
        } catch (e) {
            assert.fail(e)
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

        await Promise.all([live777VergeHost, live777CloudHost, live777LivemanHost].map(untilHttpOk))
    })

    beforeEach(async () => {
        await until(() => getStreams(live777LivemanHost), s => s.length === 0)
    })

    test("reforward", { timeout: 60 * 1000 }, async () => {
        const width = 320, height = 240
        const whipintoPort = "5003"
        const whipinto = spawn([
            appWhipinto,
            "--codec", "vp8",
            "--url", `${live777VergeHost}/whip/${live777VergeStream}`,
            "--port", whipintoPort,
        ], { onExit: e => { e.exitCode && console.log(e.stderr) } })

        const ffmpeg = spawn([
            "ffmpeg", "-hide_banner", "-re", "-f", "lavfi",
            "-i", `testsrc=size=${width}x${height}:rate=30`,
            "-vcodec", "libvpx",
            "-f", "rtp", `rtp://127.0.0.1:${whipintoPort}?pkt_size=1200`
        ], {
            stderr: null,
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        await until(() => getStreams(live777VergeHost), s => s[0]?.publish.sessions.length > 0)

        try {
            const res = await reforward(`http://127.0.0.1:${live777VergePort}`, live777VergeStream, `${live777CloudHost}/whip/${live777CloudStream}`)
            console.log(res.status === 200 ? "reforward success" : res.status)
        } catch (e) {
            assert.fail(e)
        }

        await until(() => getStreams(live777CloudHost), s => s[0]?.publish.sessions.length > 0)

        const whepfromPort = "5004"
        await writeFile(tmpFileFFplaySdp, `
v=0
m=video ${whepfromPort} RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
`)

        const whepfrom = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777CloudHost}/whep/${live777CloudStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], {
            stdout: null,
            stderr: null,
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        const res = spawnSync(["ffprobe", "-v", "error", "-hide_banner",
            "-protocol_whitelist", "file,rtp,udp", "-i", tmpFileFFplaySdp,
            "-show_format", "-show_streams", "-of", "json"])

        expect(res.status).toEqual(0)
        const ffprobe = JSON.parse(res.stdout.toString())
        expect(ffprobe.streams.length).toEqual(1)
        expect(ffprobe.streams[0].width).toEqual(width)
        expect(ffprobe.streams[0].height).toEqual(height)

        await until(() => getStreams(live777CloudHost), s => s[0]?.subscribe.sessions.length > 0)

        const streams = await getStreams(live777CloudHost)

        whepfrom.kill()
        await rm(tmpFileFFplaySdp)
        whipinto.kill()
        ffmpeg.kill()

        expect(streams).toHaveLength(1)
        expect(streams[0].subscribe.sessions).toHaveLength(1)
    })

    test("p2p to sfu", { timeout: 60 * 1000 }, async () => {
        const whipintoPort = "5005"
        const whipinto = spawn([
            appWhipinto,
            "--codec", "vp8",
            "--url", `${live777VergeHost}/whip/${live777VergeStream}`,
            "--port", whipintoPort,
        ], { onExit: e => { e.exitCode && console.log("whipinto", e.stderr) } })

        const ffmpeg = spawn([
            "ffmpeg", "-hide_banner", "-re", "-f", "lavfi",
            "-i", "testsrc=size=320x240:rate=30",
            "-vcodec", "libvpx",
            "-f", "rtp", `rtp://127.0.0.1:${whipintoPort}?pkt_size=1200`
        ], {
            stderr: null,
            onExit: e => { e.exitCode && console.log(e.stderr) },
        })

        await until(() => getStreams(live777VergeHost), s => s[0]?.publish.sessions.length > 0)
        await until(() => getStreams(live777LivemanHost), s => s[0]?.publish.sessions.length > 0)

        const whepfromPort = "5006"
        const whepfrom1 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777LivemanHost}/whep/${live777LivemanStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log("whepfrom 1", e.stderr) } })

        await until(() => getStreams(live777VergeHost), s => s[0]?.subscribe.sessions.length > 0)

        const whepfrom2 = spawn([
            appWhepfrom,
            "--codec", "vp8",
            "--url", `${live777LivemanHost}/whep/${live777LivemanStream}`,
            "--target", `127.0.0.1:${whepfromPort}`,
        ], { onExit: e => { e.exitCode && console.log("whepfrom 2", e.stderr) } })

        await Promise.all([
            until(() => getStreams(live777VergeHost), s => s[0]?.subscribe.sessions.length > 0),
            until(() => getStreams(live777CloudHost), s => s[0]?.subscribe.sessions.length > 0),
        ])

        const res1 = (await getStreams(live777VergeHost)).find(r => r.id === live777LivemanStream)
        const res2 = (await getStreams(live777CloudHost)).find(r => r.id === live777LivemanStream)

        whepfrom2.kill()
        whepfrom1.kill()
        whipinto.kill()
        ffmpeg.kill()

        expect(res1).toBeTruthy()
        expect(res1?.subscribe.sessions.length).toEqual(1)

        expect(res2).toBeTruthy()
        expect(res2?.subscribe.sessions.length).toEqual(1)
    })

    afterAll(async () => {
        await rm(tmpFileConfigVerge)
        await rm(tmpFileConfigCloud)
        await rm(tmpFileConfigMan)
        serv?.down()
    })
})
