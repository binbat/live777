import { useRef, useImperativeHandle, useState } from 'preact/hooks'
import { forwardRef } from 'preact/compat'
import { WHIPClient } from '@binbat/whip-whep/whip'

interface Props {
    onStop(): void
}

export interface IWebStreamDialog {
    show(resourceId: string): void
}

export const WebStreamDialog = forwardRef<IWebStreamDialog, Props>((props, ref) => {
    const [resourceId, setResourceId] = useState('')
    const [mediaStream, setMediaStream] = useState<MediaStream | null>()
    const [whipClient, setWhipClient] = useState<WHIPClient | null>()
    const refDialog = useRef<HTMLDialogElement>(null)
    const refVideo = useRef<HTMLVideoElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (resourceId: string) => {
                setResourceId(resourceId)
                refDialog.current?.showModal()
            }
        }
    })

    const handleCloseDialog = () => {
        refDialog.current?.close()
    }

    const handleStreamStart = async () => {
        const stream = await navigator.mediaDevices.getDisplayMedia({
            audio: true,
            video: true
        })
        setMediaStream(stream)
        if (refVideo.current) {
            refVideo.current.srcObject = stream
        }
        const pc = new RTCPeerConnection()
        pc.addTransceiver(stream.getVideoTracks()[0], { direction: 'sendonly' })
        stream.getAudioTracks().forEach(track => pc.addTrack(track))
        const whipClient = new WHIPClient()
        const url = `${location.origin}/whip/${resourceId}`
        const token = ''
        setWhipClient(whipClient)
        whipClient.publish(pc, url, token)
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

    return (
        <dialog ref={refDialog}>
            <h3>Web Stream {resourceId}</h3>
            <div>
                <video ref={refVideo} controls autoplay style={{ maxWidth: '90vw' }}></video>
            </div>
            <div>
                <button onClick={() => { handleCloseDialog() }}>Hide</button>
                {whipClient
                    ? <button onClick={() => { handleStreamStop() }} style={{ color: 'red' }}>Stop</button>
                    : <button onClick={() => { handleStreamStart() }}>Start</button>
                }
            </div>
        </dialog>
    )
})
