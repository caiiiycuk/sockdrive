import { resolve } from "path";
import { Handle, Ptr, Stats } from "./types";

declare const Module: { HEAPU8: Uint8Array };

interface Request {
    type: 1 | 2, // read | write
    sector: number,
    buffer: Ptr | Uint8Array,
    resolve: (result: number) => void,
}

export class Drive {

    sectorSize = 512;
    url: string;
    stats: Stats;

    socket: Promise<WebSocket>;
    request: Request | null;

    aheadRange: number;
    writeBuffer: Uint8Array;

    readBuffer: Uint8Array;
    readAheadBuffer: Uint8Array;
    readAheadPos: 0;
    readStartedAt: number;

    public constructor(url: string, stats: Stats, aheadRange = 64, memoryLimit = 32 * 1024 * 1024) {
        this.url = url;
        this.request = null;
        this.readBuffer = new Uint8Array(1 + 4 + 1);
        this.writeBuffer = new Uint8Array(1 + 4 + this.sectorSize);
        this.readAheadBuffer = new Uint8Array(this.sectorSize * aheadRange),
            this.stats = stats;
        this.aheadRange = aheadRange;

        this.reconnect();
    }

    public reconnect(): void {
        this.socket = new Promise<WebSocket>((resolve) => {
            const socket = new WebSocket(this.url);
            socket.binaryType = "arraybuffer";
            const onMessage = this.onMessage.bind(this);
            const onOpen = () => {
                resolve(socket);
            };
            const onError = (e: Event) => {
                console.error("Network error", e, "will reconnect");
                socket.removeEventListener("message", onMessage)
                socket.removeEventListener("open", onOpen);
                socket.removeEventListener("error", onError);
                socket.close();
                setTimeout(this.reconnect.bind(this), 300);
            };
            socket.addEventListener("message", onMessage);
            socket.addEventListener("error", onError);
            socket.addEventListener("open", onOpen);
        });

        if (this.request !== null) {
            this.executeRequest(this.request);
        }
    }

    public read(sector: number, buffer: Ptr): Promise<number> {
        return new Promise<number>((resolve) => {
            this.request = {
                type: 1,
                sector,
                buffer,
                resolve,
            };
            this.executeRequest(this.request);
        });
    }

    public write(sector: number, buffer: Ptr): Promise<number> {
        return new Promise<number>((resolve) => {
            this.request = {
                type: 2,
                sector,
                buffer: Module.HEAPU8.slice(buffer, buffer + this.sectorSize),
                resolve,
            };
            this.executeRequest(this.request);
        });
    }

    public close() {
        this.socket
            .then((s) => s.close())
            .catch((e) => console.error("Can't close socket", e));

    }

    private executeRequest(request: Request) {
        this.socket.then((socket) => {
            if (request.type === 1) {
                const { sector } = request;
                this.readStartedAt = Date.now();
                this.readAheadPos = 0;

                this.readBuffer[0] = 1; // read
                this.readBuffer[1] = sector & 0xFF;
                this.readBuffer[2] = (sector >> 8) & 0xFF;
                this.readBuffer[3] = (sector >> 16) & 0xFF;
                this.readBuffer[4] = (sector >> 24) & 0xFF;
                this.readBuffer[5] = this.aheadRange;
                socket.send(this.readBuffer.buffer);
            } else {
                const { sector, buffer, resolve } = request;
                this.stats.write += this.sectorSize;
                this.writeBuffer[0] = 2; // write
                this.writeBuffer[1] = sector & 0xFF;
                this.writeBuffer[2] = (sector >> 8) & 0xFF;
                this.writeBuffer[3] = (sector >> 16) & 0xFF;
                this.writeBuffer[4] = (sector >> 24) & 0xFF;
                // TBD: maybe do not copy and send just slice ???
                this.writeBuffer.set(buffer as Uint8Array, 5);
                socket.send(this.writeBuffer.buffer);
                resolve(0);
            }
        });
    }

    public onMessage(event: { data: ArrayBuffer }) {
        if (this.request === null) {
            console.error("Received message without request");
            this.reconnect();
        } else if (this.request.type === 2) {
            console.error("Received read payload while write request");
            this.reconnect();
        } else if (event.data instanceof ArrayBuffer) {
            const restLength = this.readAheadBuffer.length - this.readAheadPos;
            if (event.data.byteLength > restLength) {
                console.error("wrong read payload length " + event.data.byteLength + " instead of " + restLength);
            } else {
                this.readAheadBuffer.set(new Uint8Array(event.data), this.readAheadPos);
                this.readAheadPos += event.data.byteLength;

                if (this.readAheadPos == this.readAheadBuffer.length) {
                    const { buffer, resolve } = this.request;
                    Module.HEAPU8.set(new Uint8Array(this.readAheadBuffer.slice(0, this.sectorSize)), buffer as number);
                    this.stats.read += this.sectorSize * this.aheadRange;
                    this.stats.readTotalTime += Date.now() - this.readStartedAt;
                    this.request = null;
                    resolve(0);
                }
            }
        } else {
            console.error("Received non arraybuffer data");
            this.reconnect();
        }
    }

}