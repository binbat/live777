type SessionDescriptionLike = string | RTCSessionDescriptionInit;

// @params {string | RTCSessionDescriptionInit} SDP
// @params {string} audioCodec
// @params {string} videoCodec
// @return {string | RTCSessionDescriptionInit} SDP
function convertSessionDescription(
    sdp: string,
    audioCodec: string,
    videoCodec: string,
): string;
function convertSessionDescription(
    sdp: RTCSessionDescriptionInit,
    audioCodec: string,
    videoCodec: string,
): RTCSessionDescriptionInit;
function convertSessionDescription(
    sdp: SessionDescriptionLike,
    audioCodec: string,
    videoCodec: string,
): SessionDescriptionLike {
    const rawSdp = typeof sdp === "string" ? sdp : sdp.sdp;
    if (!rawSdp) {
        throw new Error("SDP is empty");
    }

    const converted = convertSdp(rawSdp, audioCodec, videoCodec);
    if (typeof sdp === "string") {
        return converted;
    }

    return {
        ...sdp,
        sdp: converted,
    };
}

function convertSdp(sdp: string, audioCodec: string, videoCodec: string): string {
    const sections = sdp.split("m=");
    for (let i = 0; i < sections.length; i++) {
        const section = sections[i];
        if (section.startsWith("audio") && !!audioCodec) {
            sections[i] = setCodec(section, audioCodec);
        } else if (section.startsWith("video") && !!videoCodec) {
            sections[i] = setCodec(section, videoCodec);
        }
    }
    return sections.join("m=");
}

function setCodec(section: string, codec: string) {
    const lines = section.split("\r\n");
    const lines2 = [];
    const payloadFormats = [];
    for (const line of lines) {
        if (!line.startsWith("a=rtpmap:")) {
            lines2.push(line);
        } else {
            if (line.toLowerCase().includes(codec.toLowerCase())) {
                payloadFormats.push(
                    line.slice("a=rtpmap:".length).split(" ")[0],
                );
                lines2.push(line);
            }
        }
    }

    if (payloadFormats.length === 0) {
        throw new Error(`Codec ${codec.toUpperCase()} is not available in SDP`);
    }

    const lines3 = [];

    for (const [index, line] of lines2.entries()) {
        if (index === 0) {
            const parts = line.split(" ");
            if (parts.length > 3) {
                lines3.push([...parts.slice(0, 3), ...payloadFormats].join(" "));
            } else {
                lines3.push(line);
            }
        } else if (line.startsWith("a=fmtp:")) {
            if (
                payloadFormats.includes(
                    line.slice("a=fmtp:".length).split(" ")[0],
                )
            ) {
                lines3.push(line);
            }
        } else if (line.startsWith("a=rtcp-fb:")) {
            if (
                payloadFormats.includes(
                    line.slice("a=rtcp-fb:".length).split(" ")[0],
                )
            ) {
                lines3.push(line);
            }
        } else {
            lines3.push(line);
        }
    }

    return lines3.join("\r\n");
}

export default convertSessionDescription;
