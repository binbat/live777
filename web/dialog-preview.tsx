import { useRef, useImperativeHandle, useState } from 'preact/hooks'
import { TargetedEvent, forwardRef } from 'preact/compat'
import { WHEPClient } from '@binbat/whip-whep/whep.js'

import { formatVideoTrackResolution } from './utils'

export interface IPreviewDialog {
    show(resourceId: string): void
}

export const PreviewDialog = forwardRef<IPreviewDialog>((_props, ref) => {
    const [resourceId, setResourceId] = useState('')
    const [whepClient, setWhepClient] = useState<WHEPClient | null>(null)
    const [videoTrack, setVideoTrack] = useState<MediaStreamTrack | null>()
    const [connState, setConnState] = useState('')
    const [videoResolution, setVideoResolution] = useState('')
    const refLogs = useRef<string[]>([])
    const refDialog = useRef<HTMLDialogElement>(null)
    const refVideo = useRef<HTMLVideoElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (resourceId: string) => {
                setResourceId(resourceId)
                handlePreviewStart(resourceId)
                refDialog.current?.showModal()
            }
        }
    })

    const handleDialogClose = async () => {
        setResourceId('')
        if (refVideo.current) {
            refVideo.current.srcObject = null
        }
        if (whepClient) {
            await whepClient.stop()
            setWhepClient(null)
        }
    }

    const log = (str: string) => {
        refLogs.current!!.push(str)
    }

    const updateConnState = (state: string) => {
        setConnState(state)
        log(state)
    }

    const handlePreviewStart = (resourceId: string) => {
        refLogs.current = []
        log('started')
        const pc = new RTCPeerConnection()
        pc.addTransceiver('video', { direction: 'recvonly' })
        pc.addTransceiver('audio', { direction: 'recvonly' })
        pc.addEventListener('track', ev => {
            log(`track: ${ev.track.kind}`)
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
        const url = `${location.origin}/whep/${resourceId}`
        const token = ''
        // @ts-ignore
        whep.onAnswer = (sdp: RTCSessionDescription) => {
            log('http answer received')
            return sdp
        }
        setWhepClient(whep)
        whep.view(pc, url, token)
        log('http offer sent')
    }

    const handleVideoResize = (_: TargetedEvent<HTMLVideoElement>) => {
        if (videoTrack) {
            setVideoResolution(formatVideoTrackResolution(videoTrack))
        }
    }

    return (
        <dialog ref={refDialog} onClose={handleDialogClose}>
            <h3>Preview {resourceId} {videoResolution}</h3>
            <div>
                <video ref={refVideo} controls autoplay onResize={handleVideoResize} style={{ maxWidth: '90vw', maxHeight: '90vh' }}></video>
            </div>
            <details>
                <summary>
                    <b>Connection Status: </b>
                    <code>{connState}</code>
                </summary>
                <pre className={'overflow-auto'} style={{ maxHeight: '10lh' }}>{refLogs.current!!.join('\n')}</pre>
            </details>
            <form method="dialog">
                <button>Close</button>
            </form>
        </dialog>
    )
})
