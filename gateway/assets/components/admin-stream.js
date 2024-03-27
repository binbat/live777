
// - stream
// - client
async function delStream(streamId, clientId) {
    return fetch(`/resource/${streamId}/${clientId}`, {
        method: "DELETE",
    })
}

async function getAllData() {
    const fetched = await fetch("/infos")
    const data = await fetched.json()
    const html = data.reduce((r, i) => r + `<tr>
<th scope="row">${i.id}</th>
<td>${i.publishLeaveTime === 0 ? "Ok" : "No"}</td>
<td>${i.subscribeSessionInfos.length}</td>
<td>${new Date(i.createTime)}</td>
<td><button onclick="delStream('${i.id}', '${i.publishSessionInfo.id}')">Destroy</button></td>
</tr>`, "")
    return html
}

export default class AdminStreamElement extends HTMLElement {
    static observedAttributes = ["autorefresh"]

    constructor() {
        super()

        this.autorefresh = false
        this.table = document.createElement("table")
        this.tbody = document.createElement("tbody")
    }

    connectedCallback() {
        const shadow = this.attachShadow({ mode: "closed" })
        const thead = document.createElement("thead")
        thead.innerHTML = `<tr>
<th>Id</th>
<th>Publisher</th>
<th>Subscriber</th>
<th>Create Time</th>
<th>Operate</th>
</tr>`

        this.table.append(thead, this.tbody)

        const button = document.createElement("button")
        button.textContent = "Auto Refresh"
        button.onclick = () => {
            !this.autorefresh ? this.setAttribute("autorefresh", !this.autorefresh) : this.removeAttribute("autorefresh")
            this.dispatchEvent(new Event("change", { composed: true }))
        }
        shadow.append(button, this.table)

        window.delStream = delStream

        this.sync()
    }

    attributeChangedCallback(name, oldValue, newValue) {
        const enabled = newValue === "true"
        this.toggleAutoRefresh(enabled)
        this.autorefresh = enabled
    }

    sync = async () => this.tbody.innerHTML = await getAllData()

    toggleAutoRefresh = (enabled) => {
        if (enabled) {
            this.timer = !!this.timer || setInterval(this.sync, 3000)
        } else {
            clearTimeout(this.timer)
            this.timer = null
        }
    }
}
