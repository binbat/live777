import convertSessionDescription from "./sdp.js"
import {WHIPClient} from "@binbat/whip-whep/whip.js"
import {WHEPClient} from "@binbat/whip-whep/whep.js"

import VideoSizeSelectElement from "./components/video-size-select.js"
import DebugPlayer from "./components/debug-player.js"
import DataChannel from "./components/data-channel.js"

customElements.define("video-size-select", VideoSizeSelectElement)
customElements.define("debug-player", DebugPlayer)
customElements.define("data-channel", DataChannel)

// Common
const idStreamId = "id"
const idBearerToken = "token"

// function setURLSearchParams(k, v) {
//     const params = new URLSearchParams((new URL(location.href)).search)
//     !!v ? params.set(k, v) : params.delete(k)
//     history.replaceState({}, "", "?" + params.toString())
// }

// function getURLSearchParams(k) {
//     const params = new URLSearchParams((new URL(location.href)).search)
//     return params.get(k)
// }

// function initCommonInput(elementId, paramId) {
//     const element = document.getElementById(elementId)
//     if (element) {
//         element.addEventListener('input', ev => setURLSearchParams(paramId, ev.target.value))
//         element.value = getURLSearchParams(paramId)
//     }
// }

// initCommonInput(idStreamId, idStreamId)
// initCommonInput(idBearerToken, idBearerToken)

function log(el, num, msg) {
    el.innerHTML += (!!num ? `[${num}]: ` : '') + msg + '<br>'
}

function logWhip(num, msg) {
    log(document.getElementById('whip-logs'), num, msg)
}

function logWhep(num, msg) {
    log(document.getElementById('whep-logs'), num, msg)
}

function getElementValue(elementId) {
    const el = document.getElementById(elementId)
    return el ? el.value : ""
}

// NOTE:
// https://github.com/livekit/client-sdk-js/blob/761711adb4195dc49a0b32e1e4f88726659dac94/src/room/track/LocalVideoTrack.ts#L492
// - f: HIGH
// - h: MEDIUM
// - q: LOW
const layers = [
    {rid: 'q', scaleResolutionDownBy: 4.0, scalabilityMode: 'L1T3'},
    {rid: 'h', scaleResolutionDownBy: 2.0, scalabilityMode: 'L1T3'},
    {rid: 'f', scalabilityMode: 'L1T3'}
]

function initLayerSelect(elementId, opts) {
    const selectLayer = document.getElementById(elementId)
    if (selectLayer) opts.map(i => {
        const opt = document.createElement("option")
        opt.value = i.value
        opt.text = i.text
        selectLayer.appendChild(opt)
    })
}

// WHIP
let whipNum = 0

const idWhipLayerSelect = "whip-layer-select"
const idWhipAudioCodec = "whip-audio-codec"
const idWhipVideoCodec = "whip-video-codec"
const idWhipAudioDevice = "whip-audio-device"
const idWhipVideoDevice = "whip-video-device"
const idWhipVideoSize = "whip-video-size"
const idWhipButtonStop = "whip-button-stop"
const idWhipPseudoAudio = "whip-pseudo-audio"
const idWhipDataChannel = "whip-datachannel"

// initLayerSelect(idWhipLayerSelect, [
//     {value: "f", text: "Base"},
//     {value: "h", text: "Base + 1/2"},
//     {value: "q", text: "Base + 1/2 + 1/4"},
// ])

async function refreshDevice() {
    const mediaStream = await navigator.mediaDevices.getUserMedia({audio: true, video: true})
    mediaStream.getTracks().map(track => track.stop())

    const devices = (await navigator.mediaDevices.enumerateDevices()).filter(i => !!i.deviceId)
    initLayerSelect(idWhipAudioDevice, devices.filter(i => i.kind === 'audioinput').map(i => {
        return {value: i.deviceId, text: i.label}
    }))
    initLayerSelect(idWhipVideoDevice, devices.filter(i => i.kind === 'videoinput').map(i => {
        return {value: i.deviceId, text: i.label}
    }))
}

window.refreshDevice = () => {
    refreshDevice()
    document.getElementById("whip-device-button").disabled = true
}

async function startWhip() {
    const streamId = getElementValue(idStreamId)
    if (!streamId) {
        alert("Please Input Stream Id")
        return
    }
    const num = whipNum++
    logWhip(num, "started")
    const videoSize = document.getElementById(idWhipVideoSize).params
    document.getElementById(idWhipVideoSize).disabled = true
    logWhip(num, `video width: ${!videoSize.width ? "default" : videoSize.width}, height: ${!videoSize.height ? "default" : videoSize.height}`)

    const audioDevice = getElementValue(idWhipAudioDevice)
    const videoDevice = getElementValue(idWhipVideoDevice)
    logWhip(num, `audio device: ${!audioDevice ? "none" : audioDevice}`)
    logWhip(num, `video device: ${!videoDevice ? "none" : videoDevice}`)

    let stream
    if (!audioDevice && !videoDevice) {
        stream = await navigator.mediaDevices.getDisplayMedia({audio: false, video: videoSize})
    } else {
        stream = await navigator.mediaDevices.getUserMedia({
            audio: {deviceId: audioDevice},
            video: {deviceId: videoDevice, ...videoSize}
        })
    }

    const el = document.getElementById("whip-video-player")
    if (el) el.srcObject = stream

    const pc = new RTCPeerConnection()

    // NOTE:
    // 1. Live777 Don't support label
    // 2. Live777 Don't support negotiated
    document.getElementById(idWhipDataChannel).dataChannel = pc.createDataChannel("")

    pc.oniceconnectionstatechange = e => logWhip(num, pc.iceConnectionState)

    const layer = getElementValue(idWhipLayerSelect)
    const index = layers.findIndex(i => i.rid === layer)

    pc.addTransceiver(stream.getVideoTracks()[0], {
        direction: 'sendonly',
        sendEncodings: layers.slice(0 - (layers.length - index)),
    })

    if (document.getElementById(idWhipPseudoAudio).checked) {
        pc.addTransceiver('audio', { 'direction': 'sendonly' })
    } else {
        stream.getAudioTracks().map(track => pc.addTrack(track))
    }

    const audioCodec = getElementValue(idWhipAudioCodec)
    document.getElementById(idWhipAudioCodec).disabled = true
    logWhip(num, `audio codec: ${!audioCodec ? "default" : audioCodec}`)

    const videoCodec = getElementValue(idWhipVideoCodec)
    document.getElementById(idWhipVideoCodec).disabled = true
    logWhip(num, `video codec: ${!videoCodec ? "default" : videoCodec}`)

    const whip = new WHIPClient()
    whip.onAnswer = answer => convertSessionDescription(answer, audioCodec, videoCodec)

    const url = location.origin + "/whip/" + streamId
    const token = getElementValue(idBearerToken)
    try {
        logWhip(num, "http begined")
        await whip.publish(pc, url, token)
    } catch (e) {
        logWhip(num, e)
    }

    const stop = async () => {
        await whip.stop()
        logWhip(num, "stopped")
        stream.getTracks().map(track => track.stop())

        if (el) el.srcObject = null
    }

    const element = document.getElementById(idWhipButtonStop)
    if (element) element.addEventListener('click', stop)

    document.getElementById(idWhipLayerSelect).disabled = true
}

window.startWhip = startWhip

// WHEP
let whepNum = 0

const idWhepLayerSelect = "whep-layer-select"
const idWhepButtonStop = "whep-button-stop"
const idWhepButtonDisableAudio = "whep-button-disable-audio"
const idWhepButtonDisableVideo = "whep-button-disable-video"
const idWhepDataChannel = "whep-datachannel"

// initLayerSelect(idWhepLayerSelect, [
//     {value: "", text: "AUTO"},
//     {value: "q", text: "LOW"},
//     {value: "h", text: "MEDIUM"},
//     {value: "f", text: "HIGH"},
// ])

async function startWhep() {
    const streamId = getElementValue(idStreamId)
    if (!streamId) {
        alert("Please Input Stream Id")
        return
    }
    const num = whepNum++
    logWhep(num, "started")
    const pc = new RTCPeerConnection()

    // NOTE:
    // 1. Live777 Don't support label
    // 2. Live777 Don't support negotiated
    document.getElementById(idWhepDataChannel).dataChannel = pc.createDataChannel("")

    pc.oniceconnectionstatechange = e => logWhep(num, pc.iceConnectionState)
    pc.addTransceiver('video', {'direction': 'recvonly'})
    pc.addTransceiver('audio', {'direction': 'recvonly'})
    pc.ontrack = ev => {
        logWhep(num, `track: ${ev.track.kind}`)
        if (ev.track.kind === "video") {
            if (ev.streams.length !== 0) document.getElementById("whep-video-player").srcObject = ev.streams[0]
        }
    }
    const whep = new WHEPClient()
    const url = location.origin + "/whep/" + streamId
    const token = getElementValue(idBearerToken)

    try {
        logWhep(num, "http begined")
        await whep.view(pc, url, token)
    } catch (e) {
        logWhep(num, e)
    }

    const element = document.getElementById(idWhepButtonStop)
    if (element) element.addEventListener('click', async () => {
        await whep.stop()
        logWhep(num, "stopped")
    })

    const buttonDisableAudio = document.getElementById(idWhepButtonDisableAudio)
    let flagButtonDisableAudio = false
    buttonDisableAudio.onclick = async () => {
        await whep.mute({ kind: "audio", enabled: flagButtonDisableAudio })
        buttonDisableAudio.innerText = flagButtonDisableAudio ? "Disable Audio" : "Enable Audio"
        flagButtonDisableAudio = !flagButtonDisableAudio
    }

    const buttonDisableVideo = document.getElementById(idWhepButtonDisableVideo)
    let flagButtonDisableVideo = false
    buttonDisableVideo.onclick = async () => {
        await whep.mute({ kind: "video", enabled: flagButtonDisableVideo })
        buttonDisableVideo.innerText = flagButtonDisableVideo ? "Disable Video" : "Enable Video"
        flagButtonDisableVideo = !flagButtonDisableVideo
    }

    const initEvevt = () => {
        const el = document.getElementById(idWhepLayerSelect)
        if (el) el.onchange = ev => !ev.target.value ? whep.unselectLayer() : whep.selectLayer({"encodingId": ev.target.value}).catch(e => logWhep(e))
    }

    if (whep.layerUrl) {
        const selectLayer = document.getElementById(idWhepLayerSelect)
        if (selectLayer) selectLayer.disabled = false
        initEvevt()
    }
}

window.startWhep = startWhep
