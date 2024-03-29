// @params {string} SDP
// @params {string} audioCodec
// @params {string} videoCodec
// @return {string} SDP
function convertSessionDescription(sdp, audioCodec, videoCodec) {
    const sections = sdp.split("m=")
    for (let i = 0; i < sections.length; i++) {
        const section = sections[i]
        if (section.startsWith("audio") && !!audioCodec) {
            sections[i] = setCodec(section, audioCodec)
        } else if (section.startsWith("video") && !!videoCodec) {
            sections[i] = setCodec(section, videoCodec)
        }
    }
    return sections.join("m=")
}

function setCodec(section, codec) {
    const lines = section.split("\r\n")
    const lines2 = []
    const payloadFormats = []
    for (const line of lines) {
        if (!line.startsWith("a=rtpmap:")) {
            lines2.push(line)
        } else {
            if (line.toLowerCase().includes(codec)) {
                payloadFormats.push(line.slice("a=rtpmap:".length).split(" ")[0])
                lines2.push(line)
            }
        }
    }

    const lines3 = []

    for (const line of lines2) {
        if (line.startsWith("a=fmtp:")) {
            if (payloadFormats.includes(line.slice("a=fmtp:".length).split(" ")[0])) {
                lines3.push(line)
            }
        } else if (line.startsWith("a=rtcp-fb:")) {
            if (payloadFormats.includes(line.slice("a=rtcp-fb:".length).split(" ")[0])) {
                lines3.push(line)
            }
        } else {
            lines3.push(line)
        }
    }

    return lines3.join("\r\n")
}

export default convertSessionDescription
