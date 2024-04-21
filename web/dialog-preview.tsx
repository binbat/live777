import { useRef, useImperativeHandle, useState } from 'preact/hooks'
import { forwardRef } from 'preact/compat'
import { WHEPClient } from "@binbat/whip-whep/whep.js"

export interface IPreviewDialog {
    show(resourceId: string): void
}

export const PreviewDialog = forwardRef<IPreviewDialog>((_props, ref) => {
    const [resourceId, setResourceId] = useState('')
    const [whepClient, setWhepClient] = useState<WHEPClient | null>(null)
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

    const handlePreviewStart = (resourceId: string) => {
        const pc = new RTCPeerConnection()
        pc.addTransceiver('video', { 'direction': 'recvonly' })
        pc.addTransceiver('audio', { 'direction': 'recvonly' })
        pc.ontrack = ev => {
            if (ev.track.kind === "video" && ev.streams.length > 0) {
                if (refVideo.current) {
                    refVideo.current.srcObject = ev.streams[0]
                }
            }
        }
        const whep = new WHEPClient()
        const url = location.origin + "/whep/" + resourceId
        const token = ''
        setWhepClient(whep)
        whep.view(pc, url, token)
    }

    return (
        <dialog ref={refDialog} onClose={handleDialogClose}>
            <h3>Preview {resourceId}</h3>
            <div>
                <video ref={refVideo} controls autoplay></video>
            </div>
            <form method="dialog">
                <button>Close</button>
            </form>
        </dialog>
    )
})
