export default class DataChannelElement extends HTMLElement {
    constructor() {
        super()

        this.dc = null
        this.samp = document.createElement("samp")
        this.input = document.createElement("input")
        this.button = document.createElement("button")
        this.log = document.createElement("div")
    }

    connectedCallback() {
        const shadow = this.attachShadow({ mode: "closed" })
        const label = document.createElement("label")
        label.textContent = "Receive messages:"
        this.input.type = "text"
        this.input.placeholder = "Input message text"
        this.input.disabled = true
        this.button.textContent = "Send"
        this.button.disabled = true
        this.button.onclick = () => this.dc.send(this.input.value)

        shadow.append(this.samp, this.input, this.button, document.createElement("br"), label, this.log)
    }

    // @params DataChannel
    set dataChannel(dc) {
        if (this.dc) this.dc.removeEventListener("open", this.onopen)
        if (this.dc) this.dc.removeEventListener("close", this.onclose)
        if (this.dc) this.dc.removeEventListener("message", this.onmessage)
        this.dc = dc
        this.dc.addEventListener("message", this.onmessage)
        this.dc.addEventListener("close", this.onclose)
        this.dc.addEventListener("open", this.onopen)
    }

    onopen = () => {
        this.input.disabled = false
        this.button.disabled = false
    }
    onclose = () => {
        this.input.disabled = true
        this.button.disabled = false
    }
    onmessage = ev => this.log.innerHTML += (new TextDecoder('utf-8')).decode(new Uint8Array(ev.data)) + '<br>'
}
