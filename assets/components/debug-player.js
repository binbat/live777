export default class VideoSizeSelectElement extends HTMLElement {
    constructor() {
        super()
        this.labelCount = document.createElement("label")
        this.labelSize = document.createElement("label")
        this.video = document.createElement("video")
        this.video.autoplay = true
        this.video.controls = true
        this.video.addEventListener("loadedmetadata", this.setShowSize)
    }

    connectedCallback() {
        const shadow = this.attachShadow({ mode: "closed" })

        const selectWidthValue = document.createElement("select")
        selectWidthValue.onchange = ev => this.video.style.width = ev.target.value

        const addSelectOptions = (arr, root) => arr.map(i => {
            const option = document.createElement("option")
            option.value = i
            option.innerText = i
            root.append(option)
        })
        addSelectOptions(["auto", "320px", "480px", "600px", "1280px", "1920px"], selectWidthValue)

        const labelWidthHeight = document.createElement("label")

        shadow.append(labelWidthHeight)
        shadow.append(this.labelCount)
        shadow.append(document.createElement("br"))
        shadow.append(this.labelSize)
        shadow.append(document.createElement("br"))
        shadow.append(selectWidthValue)
        shadow.append(document.createElement("br"))
        shadow.append(this.video)
    }

    // @params MediaStream
    // @return void
    set srcObject(stream) {
        if (this.stream) this.stream.removeEventListener("addtrack", this.setShowTrackCount)
        if (this.stream) this.stream.removeEventListener("removetrack", this.setShowTrackCount)
        this.stream = stream
        this.stream.addEventListener("addtrack", this.setShowTrackCount)
        this.stream.addEventListener("removetrack", this.setShowTrackCount)
        this.setShowTrackCount(stream)
        this.video.srcObject = stream
    }

    setShowSize = () => this.labelSize.innerText = `Raw Resolution: ${this.video.videoWidth}x${this.video.videoHeight}`
    setShowTrackCount = (ev) => {
        //const stream = ev.target
        const stream = this.stream
        this.labelCount.innerText = `Audio Track Count: ${stream.getAudioTracks().length}, Video Track Count: ${stream.getVideoTracks().length}`
    }
}
