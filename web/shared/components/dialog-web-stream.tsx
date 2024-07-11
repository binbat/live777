import { useRef, useImperativeHandle, useState } from 'preact/hooks'
import { TargetedEvent, forwardRef } from 'preact/compat'
import { WHIPClient } from '@binbat/whip-whep/whip'

import { formatVideoTrackResolution } from '../utils'
import { useLogger } from '../hooks/use-logger'

interface Props {
    onStop(): void
}

export interface IWebStreamDialog {
    show(streamId: string): void
}

export const WebStreamDialog = forwardRef<IWebStreamDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState('')
    const [mediaStream, setMediaStream] = useState<MediaStream | null>()
    const [whipClient, setWhipClient] = useState<WHIPClient | null>()
    const [connState, setConnState] = useState('')
    const [videoResolution, setVideoResolution] = useState('')
    const logger = useLogger()
    const refDialog = useRef<HTMLDialogElement>(null)
    const refVideo = useRef<HTMLVideoElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setStreamId(streamId)
                refDialog.current?.showModal()
            }
        }
    })

    const handleCloseDialog = () => {
        refDialog.current?.close()
    }

    const updateConnState = (state: string) => {
        setConnState(state)
        logger.log(state)
    }

    const handleStreamStart = async () => {
        logger.clear()
        setConnState('')
        const stream = await navigator.mediaDevices.getDisplayMedia({
            audio: true,
            video: true
        })
        setMediaStream(stream)
        if (refVideo.current) {
            refVideo.current.srcObject = stream
        }
        const videoTrack = stream.getVideoTracks()[0]
        setVideoResolution(formatVideoTrackResolution(videoTrack))
        updateConnState('Started')
        const pc = new RTCPeerConnection()
        pc.addEventListener('iceconnectionstatechange', () => {
            updateConnState(pc.iceConnectionState)
        })
        pc.addTransceiver(videoTrack, { direction: 'sendonly' })
        stream.getAudioTracks().forEach(track => pc.addTrack(track))
        const whip = new WHIPClient()
        const url = `${location.origin}/whip/${streamId}`
        const token = ''
        whip.onOffer = sdp => {
            logger.log('http offer sent')
            return sdp
        }
        whip.onAnswer = sdp => {
            logger.log('http answer received')
            return sdp
        }
        setWhipClient(whip)
        try {
            await whip.publish(pc, url, token)
        } catch (e: any) {
            setConnState('Error')
            if (e instanceof Error) {
                logger.log(e.message)
            }
            const r = e.response as Response | undefined
            if (r) {
                logger.log(await r.text())
            }
        }
    }

    const handleStreamStop = async () => {
        if (mediaStream) {
            mediaStream.getTracks().forEach(t => t.stop())
            setMediaStream(null)
        }
        if (refVideo.current) {
            refVideo.current.srcObject = null
        }
        if (whipClient) {
            await whipClient.stop()
            setWhipClient(null)
        }
        props.onStop()
        handleCloseDialog()
    }

    const handleVideoResize = (_: TargetedEvent<HTMLVideoElement>) => {
        const videoTrack = mediaStream?.getVideoTracks()[0]
        if (videoTrack) {
            setVideoResolution(formatVideoTrackResolution(videoTrack))
        }
    }

    return (
        <dialog ref={refDialog}>
            <h3>Web Stream {streamId} {videoResolution}</h3>
            <div>
                <video ref={refVideo} controls autoplay onResize={handleVideoResize} class="max-w-[90vw] max-h-[70vh]"></video>
            </div>
            <details>
                <summary>
                    <b>Connection Status: </b>
                    <code>{connState}</code>
                </summary>
                <pre class="overflow-auto max-h-[10lh]">{logger.logs.join('\n')}</pre>
            </details>
            <div>
                <button onClick={() => { handleCloseDialog() }}>Hide</button>
                {whipClient
                    ? <button onClick={() => { handleStreamStop() }} class="text-red-500">Stop</button>
                    : <button onClick={() => { handleStreamStart() }}>Start</button>
                }
            </div>
        </dialog>
    )
})
