
// - stream
// - client
async function delStream(streamId: string, clientId: string) {
    return fetch(`/resource/${streamId}/${clientId}`, {
        method: "DELETE",
    })
}

async function allStream(): Promise<any[]> {
    return (await fetch("/admin/infos")).json()
}

async function reforward(streamId: string, url: string): Promise<void> {
    fetch(`/admin/reforward/${streamId}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
        },
        body: JSON.stringify({
            targetUrl: url,
        }),
    })
}

export {
    allStream,
    delStream,
    reforward,
}
