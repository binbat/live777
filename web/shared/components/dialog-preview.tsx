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
    const [whepClient, setWhepClient] = useState<WHEPClient | null>(null)
    const [videoTrack, setVideoTrack] = useState<MediaStreamTrack | null>()
    const [connState, setConnState] = useState('')
    const [videoResolution, setVideoResolution] = useState('')
    const logger = useLogger()
    const refDialog = useRef<HTMLDialogElement>(null)
    const refVideo = useRef<HTMLVideoElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: async (newStreamId: string) => {
                if (streamId !== newStreamId) {
                    if (streamId !== '' && whepClient !== null) {
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
        if (whepClient) {
            await whepClient.stop()
            setWhepClient(null)
        }
        props.onStop()
        handleCloseDialog()
    }

    const updateConnState = (state: string) => {
        setConnState(state)
        logger.log(state)
    }

    const handlePreviewStart = (streamId: string) => {
        logger.clear()
        logger.log('started')
        const pc = new RTCPeerConnection()
        pc.addTransceiver('video', { direction: 'recvonly' })
        pc.addTransceiver('audio', { direction: 'recvonly' })
        pc.addEventListener('track', ev => {
            logger.log(`track: ${ev.track.kind}`)
            if (ev.track.kind === 'video' && ev.streams.length > 0) {
                setVideoTrack(ev.track)
                if (refVideo.current) {
                    refVideo.current.srcObject = ev.streams[0]
                }
            }
        })
        pc.addEventListener('iceconnectionstatechange', () => {
            updateConnState(pc.iceConnectionState)
        })
        const whep = new WHEPClient()
        const url = `${location.origin}/whep/${streamId}`
        const token = ''
        // @ts-ignore
        whep.onAnswer = (sdp: RTCSessionDescription) => {
            logger.log('http answer received')
            return sdp
        }
        setWhepClient(whep)
        whep.view(pc, url, token)
        logger.log('http offer sent')
    }

    const handleVideoResize = (_: TargetedEvent<HTMLVideoElement>) => {
        if (videoTrack) {
            setVideoResolution(formatVideoTrackResolution(videoTrack))
        }
    }

    return (
        <dialog ref={refDialog}>
            <h3>Preview {streamId} {videoResolution}</h3>
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
            <form method="dialog">
                <button onClick={() => handleCloseDialog()}>Hide</button>
                <button onClick={() => handlePreviewStop()} class="text-red-500">Stop</button>
            </form>
        </dialog>
    )
})
