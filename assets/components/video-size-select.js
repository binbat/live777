export default class VideoSizeSelectElement extends HTMLElement {
    constructor() {
        super()
        this.width = "320"
        this.height = "240"

        // Reference: https://developer.mozilla.org/en-US/docs/Web/API/Media_Capture_and_Streams_API/Constraints
        this.params = {}

        this.enabledWidth = document.createElement("input")
        this.enabledWidth.type = "checkbox"
        this.enabledHeight = document.createElement("input")
        this.enabledHeight.type = "checkbox"
    }

    connectedCallback() {
        const shadow = this.attachShadow({ mode: "closed" })

        const fieldset = document.createElement("fieldset")
        fieldset.style.borderStyle = "unset"
        fieldset.style.padding = "unset"

        const labelWidth = document.createElement("label")
        const selectWidthValue = document.createElement("select")

        const labelHeight = document.createElement("label")
        const selectHeightValue = document.createElement("select")

        labelWidth.innerText = "Width: "
        selectWidthValue.disabled = true
        this.enabledWidth.onclick = ev => {
            if (ev.target.checked) {
                selectWidthValue.disabled = false
                this.params.width = this.width
            } else {
                selectWidthValue.disabled = true
                this.width = this.params.width
                delete this.params.width
            }
        }

        labelHeight.innerText = "Height: "
        selectHeightValue.disabled = true
        this.enabledHeight.onclick = ev => {
            if (ev.target.checked) {
                selectHeightValue.disabled = false
                this.params.height = this.height
            } else {
                selectHeightValue.disabled = true
                this.height = this.params.height
                delete this.params.height
            }
        }

        const addSelectOptions = (arr, root) => arr.map(i => {
            const option = document.createElement("option")
            option.value = i
            option.innerText = i
            root.append(option)
        })

        selectWidthValue.onchange = ev => this.params.width = ev.target.value
        selectHeightValue.onchange = ev => this.params.height = ev.target.value

        addSelectOptions(["320", "480", "600", "1280", "1920", "3480"], selectWidthValue)
        addSelectOptions(["240", "320", "480", "720", "1080", "2160"], selectHeightValue)

        shadow.append(fieldset)

        fieldset.append(this.enabledWidth)
        fieldset.append(labelWidth)
        fieldset.append(selectWidthValue)

        fieldset.append(this.enabledHeight)
        fieldset.append(labelHeight)
        fieldset.append(selectHeightValue)
    }

    // @params boolean
    // @return void
    set disabled(value) {
        this.enabledWidth.disabled = value
        this.enabledHeight.disabled = value
    }
}
