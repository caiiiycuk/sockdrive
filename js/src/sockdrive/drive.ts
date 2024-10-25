import { Cache } from "./cache";
import { decodeFrame, Frame } from "./decode";
import { EmModule, Ptr, Stats } from "./types";

interface Request {
    type: 1 | 2, // read | write
    sector: number,
    buffer: Ptr | Uint8Array,
    resolve: (result: number) => void,
}

export class Drive {
    module: EmModule;
    sectorSize: number;
    endpoint: string;
    owner: string;
    drive: string;
    realOwner: string;
    realDrive: string;
    token: string;
    stats: Stats;

    socket: Promise<WebSocket> = Promise.resolve(null as any);
    request: Request | null;

    frame: Frame;

    aheadRange: number = 0;
    aheadSize: number = 0;
    writeBuffer: Uint8Array;

    readBuffered: boolean;
    readBuffer: Uint8Array;
    readStartedAt: number = 0;

    cache: Cache | null;
    cleanup = () => {/**/};

    openFn = (read: boolean, write: boolean, size: number, aheadRange: number,
        realOwner: string, realName: string) => {/**/};
    errorFn = (e: Error) => {/**/};

    readOnly = false;
    alive = true;
    lastBufferedAmount = 0;

    statsDirty: { [sector: number]: number } = {};
    statsCommitInterval: number;

    public constructor(endpoint: string,
        owner: string,
        drive: string,
        token: string,
        stats: Stats,
        module: EmModule,
        cache: Cache | null = null,
        readBuffered: boolean = false) {
        this.sectorSize = 512;
        this.module = module;
        this.endpoint = endpoint;
        this.owner = owner;
        this.drive = drive;
        this.realOwner = owner;
        this.realDrive = drive;
        this.token = token;
        this.request = null;
        this.readBuffer = new Uint8Array(1 + 4 + 4);
        this.writeBuffer = new Uint8Array(1 + 4 + this.sectorSize);
        this.stats = stats;
        this.cache = cache;
        this.cache?.open(this.owner, this.drive, this.token);
        this.frame = null as any;
        this.readBuffered = readBuffered;
        this.reconnect();

        this.statsCommitInterval = setInterval(() => {
            if (Object.keys(this.statsDirty).length > 0) {
                fetch(endpoint.replace("wss://", "https://").replace("ws://", "http://") + "/cache/set" +
                    "/" + encodeURIComponent(this.realOwner) +
                    "/" + encodeURIComponent(this.realDrive) +
                    "/" + (token.length === 0 ? "guest" : token),
                {
                    method: "POST",
                    headers: {
                        "Content-Type": "application/json",
                    },
                    body: JSON.stringify(this.statsDirty),
                }).catch((e) => console.warn("Can't send drive cache stats", e));
                this.statsDirty = {};
            }
        }, 1000 * 60) as any;
    }

    public onError(errorFn: (e: Error) => void) {
        this.errorFn = errorFn;
    }

    public onOpen(openFn: (read: boolean, write: boolean, imageSize: number, aheadRange: number,
        realOwner: string, realName: string) => void) {
        this.openFn = openFn;
    }

    public reconnect(): void {
        if (!this.alive) {
            return;
        }

        if (this.socket) {
            this.socket.then((s) => {
                if (s !== null) {
                    s.close();
                }
            });
        }

        this.socket = new Promise<WebSocket>((resolve, reject) => {
            const socket = new WebSocket(this.endpoint);
            socket.binaryType = "arraybuffer";
            const onMessage = this.onMessage.bind(this);
            const onOpen = () => {
                const onInit = (event: { data: string }) => {
                    socket.removeEventListener("message", onInit);
                    if (event.data.startsWith("write") || event.data.startsWith("read")) {
                        const [mode, aheadRangeStr, sizeStr, realOwner, realDrive] = event.data.split(",");
                        this.realOwner = realOwner;
                        this.realDrive = realDrive;
                        this.aheadRange = Number.parseInt(aheadRangeStr);
                        this.aheadSize = this.aheadRange * this.sectorSize;
                        this.readOnly = mode !== "write";

                        this.frame = {
                            sectors: [],
                            sectorsRow: new Uint8Array(this.aheadSize),
                            payload: new Uint8Array(this.aheadSize),
                            payloadPos: 0,
                            payloadSize: 0,
                            aheadSize: this.aheadSize,
                        };

                        const onPreloadMessage = (event: { data: ArrayBuffer }) => {
                            socket.removeEventListener("message", onPreloadMessage);
                            socket.addEventListener("message", onMessage);
                            this.openFn(true, !this.readOnly,
                                (Number.parseInt(sizeStr) ?? 2 * 1024 * 1024) * 1024,
                                this.aheadRange, realOwner, realDrive);

                            this.onOpen = () => { };
                            this.errorFn = () => { };
                            resolve(socket);
                        };
                        socket.addEventListener("message", onPreloadMessage);
                    } else {
                        const error = new Error(event.data ?? "Unable to establish connection");
                        this.errorFn(error);
                        reject(error);
                    }
                };
                socket.addEventListener("message", onInit);
                socket.send(this.owner + "&" + this.drive + "&" + this.token);
            };
            const onError = (e: Event) => {
                console.error("Network error", e, "will reconnect");
                cleanup();
                socket.close();
                setTimeout(this.reconnect.bind(this), 300);
            };
            socket.addEventListener("error", onError);
            socket.addEventListener("open", onOpen);
            const cleanup = function() {
                socket.removeEventListener("message", onMessage);
                socket.removeEventListener("open", onOpen);
                socket.removeEventListener("error", onError);
            };
            this.cleanup = cleanup;
        });

        if (this.request !== null) {
            this.executeRequest(this.request);
        }
    }

    public read(sector: number, buffer: Ptr, sync: boolean): Promise<number> | number {
        const origin = this.getOrigin(sector);
        this.statsDirty[origin] = (this.statsDirty[origin] ?? 0) + 1;

        const cached = this.cache?.read(this.owner, this.drive, sector);
        if (cached) {
            this.stats.cacheHit++;
            this.module.HEAPU8.set(this.readBuffered || cached.length == this.sectorSize ? cached :
                cached.slice(0, this.sectorSize), buffer);
            return sync ? 0 : Promise.resolve(0);
        } else if (sync) {
            return 255;
        } else {
            return new Promise<number>((resolve) => {
                this.stats.cacheMiss++;

                const request: Request = {
                    type: 1,
                    sector,
                    buffer,
                    resolve,
                };

                if (this.request !== null) {
                    console.error("New read request while old one is still processed");
                    resolve(3);
                } else {
                    this.request = request;
                    this.executeRequest(this.request);
                }
            });
        }
    }

    public write(sector: number, buffer: Ptr): number {
        const request: Request = {
            type: 2,
            sector,
            buffer: this.module.HEAPU8.slice(buffer, buffer + this.sectorSize),
            resolve: () => {/**/},
        };
        this.lastBufferedAmount += this.sectorSize;
        this.cache?.write(this.owner, this.drive, sector, request.buffer as Uint8Array);
        this.executeRequest(request);
        return 0;
    }

    public async close() {
        clearInterval(this.statsCommitInterval);
        this.alive = false;
        const socket = await this.socket;
        await new Promise<void>((resolve) => {
            const intervalId = setInterval(() => {
                if (socket.bufferedAmount === 0) {
                    clearInterval(intervalId);
                    resolve();
                }
            }, 32);
        });
        this.cleanup();
        socket.close();
    }

    private executeRequest(request: Request) {
        this.socket.then(async (socket) => {
            if (socket.readyState === WebSocket.CLOSED || socket.readyState === WebSocket.CLOSING) {
                if (!this.alive) {
                    console.error("Trying to read from closed drive", this.drive);
                    request.resolve(4);
                } else {
                    console.error("Drive connection to '" + this.owner + "/" + this.drive +
                        "' was closed, trying to reconnect...");
                    this.reconnect();
                }
                return;
            }

            if (request.type === 1) {
                const { sector } = request;
                this.readStartedAt = Date.now();
                this.readBuffer[0] = 1; // read
                this.readBuffer[1] = 1 & 0xFF;
                this.readBuffer[2] = (1 >> 8) & 0xFF;
                this.readBuffer[3] = (1 >> 16) & 0xFF;
                this.readBuffer[4] = (1 >> 24) & 0xFF;

                const origin = this.getOrigin(sector);
                this.readBuffer[5] = origin & 0xFF;
                this.readBuffer[5 + 1] = (origin >> 8) & 0xFF;
                this.readBuffer[5 + 2] = (origin >> 16) & 0xFF;
                this.readBuffer[5 + 3] = (origin >> 24) & 0xFF;
                this.frame.sectors = [sector];
                this.frame.payloadPos = 0;
                this.frame.payloadSize = 0;
                socket.send(this.readBuffer.slice(0, 4 + 5).buffer);
            } else if (this.readOnly) {
                request.resolve(0);
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
        }).catch(console.error);
    }

    public onMessage(event: { data: ArrayBuffer }) {
        if (this.request === null) {
            console.error("Received message without request");
            this.reconnect();
        } else if (this.request.type === 2) {
            console.error("Received read payload while write request");
            this.reconnect();
        } else if (decodeFrame(event, this.frame)) {
            if (this.frame.error) {
                console.error(this.frame.error);
                this.reconnect();
                return;
            } else {
                const { buffer, sector, resolve } = this.request;
                const origin = this.getOrigin(sector);
                this.cache?.update(this.owner, this.drive, origin, this.frame.sectorsRow, 0);
                const from = (sector - origin) * this.sectorSize;
                if (this.readBuffered) {
                    if (from !== 0) {
                        console.error("Buffered mode only works for origin!");
                        resolve(3);
                    } else {
                        this.module.HEAPU8.set(this.frame.sectorsRow, buffer as number);
                    }
                } else {
                    this.module.HEAPU8.set(this.frame.sectorsRow.slice(from, from + this.sectorSize), buffer as number);
                }

                this.stats.read += this.frame.payloadSize;
                this.stats.readTotalTime += Date.now() - this.readStartedAt;
                this.request = null;
                resolve(0);
            }
        }
    }

    public currentSocket() {
        return this.socket;
    }

    public getOrigin(sector: number) {
        return sector - sector % this.aheadRange;
    }

    public bufferedAmount(): number {
        this.socket.then((s) => {
            if (s) {
                this.lastBufferedAmount = s.bufferedAmount;
            }
        });
        return this.lastBufferedAmount;
    }
}
