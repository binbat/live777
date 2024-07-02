import { useRef, useImperativeHandle, useState } from 'preact/hooks'
import { TargetedEvent, forwardRef } from 'preact/compat'
import { WHEPClient } from '@binbat/whip-whep/whep.js'

import { formatVideoTrackResolution } from '../utils'
import { useLogger } from '../hooks/use-logger'

interface Props {
    onStop(): void
}

export interface IPreviewDialog {
    show(streamId: string): void
}

export const PreviewDialog = forwardRef<IPreviewDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState('')
    const refPeerConnection = useRef<RTCPeerConnection | null>(null)
    const refWhepClient = useRef<WHEPClient | null>(null)
    const refVideoTrack = useRef<MediaStreamTrack | null>(null)
    const [connState, setConnState] = useState('')
    const [videoResolution, setVideoResolution] = useState('')
    const logger = useLogger()
    const refDialog = useRef<HTMLDialogElement>(null)
    const refVideo = useRef<HTMLVideoElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: async (newStreamId: string) => {
                if (streamId !== newStreamId) {
                    if (streamId !== '' && refWhepClient.current !== null) {
                        await handlePreviewStop()
                    }
                    setStreamId(newStreamId)
                    handlePreviewStart(newStreamId)
                }
                refDialog.current?.showModal()
            }
        }
    })

    const handleCloseDialog = () => {
        refDialog.current?.close()
    }

    const handlePreviewStop = async () => {
        if (refVideo.current) {
            refVideo.current.srcObject = null
        }
        if (refVideoTrack.current) {
            refVideoTrack.current = null
        }
        if (refPeerConnection.current) {
            refPeerConnection.current = null
        }
        if (refWhepClient.current) {
            await refWhepClient.current.stop()
            refWhepClient.current = null
        }
        props.onStop()
        handleCloseDialog()
    }

    const updateConnState = (state: string) => {
        setConnState(state)
        logger.log(state)
    }

    const logInboundRtpStats = async () => {
        const stats = await refPeerConnection.current?.getStats() ?? null
        if (!stats) return
        for (const [_, s] of stats) {
            if (s.type === 'inbound-rtp') {
                const { id, bytesReceived } = s as RTCInboundRtpStreamStats
                // log the first time bytesReceived is not 0
                if (!bytesReceived) {
                    window.queueMicrotask(logInboundRtpStats)
                } else {
                    logger.log(`inbound-rtp(${id}): ${bytesReceived} bytes`)
                }
            }
        }
    }

    const handlePreviewStart = async (streamId: string) => {
        logger.clear()
        updateConnState('Started')
        const pc = new RTCPeerConnection()
        pc.addTransceiver('video', { direction: 'recvonly' })
        pc.addTransceiver('audio', { direction: 'recvonly' })
        pc.addEventListener('track', ev => {
            logger.log(`track: ${ev.track.kind}`)
            if (ev.track.kind === 'video' && ev.streams.length > 0) {
                refVideoTrack.current = ev.track
                if (refVideo.current) {
                    refVideo.current.srcObject = ev.streams[0]
                }
            }
        })
        pc.addEventListener('iceconnectionstatechange', () => {
            const state = pc.iceConnectionState
            updateConnState(state)
            if (state === 'connected') {
                window.queueMicrotask(logInboundRtpStats)
            }
        })
        refPeerConnection.current = pc
        const whep = new WHEPClient()
        const url = `${location.origin}/whep/${streamId}`
        const token = ''
        whep.onOffer = sdp => {
            logger.log('http offer sent')
            return sdp
        }
        whep.onAnswer = sdp => {
            logger.log('http answer received')
            return sdp
        }
        refWhepClient.current = whep
        try {
            await whep.view(pc, url, token)
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

    const handleVideoCanPlay = (_: TargetedEvent<HTMLVideoElement>) => {
        logger.log('video canplay')
    }

    const handleVideoResize = (_: TargetedEvent<HTMLVideoElement>) => {
        if (refVideoTrack.current) {
            setVideoResolution(formatVideoTrackResolution(refVideoTrack.current))
        }
    }

    return (
        <dialog ref={refDialog}>
            <h3>Preview {streamId} {videoResolution}</h3>
            <div>
                <video ref={refVideo} controls autoplay onCanPlay={handleVideoCanPlay} onResize={handleVideoResize} class="max-w-[90vw] max-h-[70vh]"></video>
            </div>
            <details>
                <summary>
                    <b>Connection Status: </b>
                    <code>{connState}</code>
                </summary>
                <pre class="overflow-auto max-h-[10lh]">{logger.logs.join('\n')}</pre>
            </details>
            <form method="dialog">
                <button onClick={() => handleCloseDialog()}>Hide</button>
                <button onClick={() => handlePreviewStop()} class="text-red-500">Stop</button>
            </form>
        </dialog>
    )
})
