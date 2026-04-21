import QrSender from "./qr-sender";
import QrReceiver from "./qr-receiver";

export default function QrLatency() {
    return (
        <fieldset>
            <legend>QR Latency</legend>
            <div style="display: flex; justify-content: space-evenly; flex-wrap: wrap;">
                <QrSender />
                <QrReceiver />
            </div>
        </fieldset>
    );
}
