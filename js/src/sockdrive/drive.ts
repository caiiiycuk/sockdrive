import { EmModule, Ptr, Stats } from "./types";
import { BlockCache, Cache } from "./cache";

const MAX_FRAME_SIZE = 1 * 1024 * 1024;
const MEMORY_LIMIT = 64 * 1024 * 1024;

interface Request {
    type: 1 | 2, // read | write
    sectors: number[],
    buffer: Ptr | Uint8Array,
    resolve: (result: number) => void,
}

export class Drive {
    module: EmModule;
    sectorSize: number;
    endpoint: string;
    owner: string;
    drive: string;
    token: string;
    stats: Stats;

    socket: Promise<WebSocket> = Promise.resolve(null as any);
    request: Request | null;
    pendingRequest: Request | null;

    aheadRange: number = 0;
    aheadSize: number = 0;
    writeBuffer: Uint8Array;

    maxRead: number = 0;
    readBuffer: Uint8Array = new Uint8Array(0);
    readAheadBuffer: Uint8Array = new Uint8Array(0);
    readAheadPos = 0;
    readAheadCompressed = 0;
    readStartedAt: number = 0;
    decodeBuffer: Uint8Array = new Uint8Array(0);

    cache: Cache | null = null;
    cleanup = () => {/**/};

    openFn = (read: boolean, write: boolean, size: number, preloadQueue: number[],
        aheadRange: number) => {/**/};
    preloadProgressFn = (restBytes: number) => {/**/};
    errorFn = (e: Error) => {/**/};

    retries: number;

    preloadSectors: boolean;
    preloadQueue: number[] = [];

    originReadMode: boolean;
    readOnly = false;

    public constructor(endpoint: string,
        owner: string,
        drive: string,
        token: string,
        stats: Stats,
        module: EmModule,
        preloadSectors = true,
        originReadMode = false) {
        this.sectorSize = 512;
        this.module = module;
        this.endpoint = endpoint;
        this.owner = owner;
        this.drive = drive;
        this.token = token;
        this.request = null;
        this.pendingRequest = null;
        this.writeBuffer = new Uint8Array(1 + 4 + this.sectorSize);
        this.stats = stats;
        this.retries = 3;
        this.preloadSectors = preloadSectors;
        this.originReadMode = originReadMode;

        this.reconnect();
    }

    public onError(errorFn: (e: Error) => void) {
        this.errorFn = errorFn;
    }

    public onOpen(openFn: (read: boolean, write: boolean, imageSize: number,
        preloadQueue: number[], aheadRange: number) => void) {
        this.openFn = openFn;
    }

    public onPreloadProgress(progressFn: (restBytes: number) => void) {
        this.preloadProgressFn = progressFn;
    }

    public reconnect(): void {
        this.socket = new Promise<WebSocket>((resolve, reject) => {
            const socket = new WebSocket(this.endpoint);
            socket.binaryType = "arraybuffer";
            const onMessage = this.onMessage.bind(this);
            const onOpen = () => {
                const onInit = (event: { data: string }) => {
                    socket.removeEventListener("message", onInit);
                    if (event.data.startsWith("write") || event.data.startsWith("read")) {
                        const [mode, aheadRangeStr, sizeStr] = event.data.split(",");
                        const aheadRange = Number.parseInt(aheadRangeStr);
                        this.aheadRange = aheadRange;
                        this.aheadSize = aheadRange * this.sectorSize;
                        this.maxRead = Math.floor(MAX_FRAME_SIZE / this.aheadSize);

                        const decodeBufferSize = this.aheadSize * this.maxRead;
                        this.readBuffer = new Uint8Array(1 + 4 + this.maxRead * 4);
                        this.readAheadBuffer = new Uint8Array(decodeBufferSize);
                        this.decodeBuffer = new Uint8Array(decodeBufferSize);
                        this.cache = new BlockCache(this.sectorSize, aheadRange, MEMORY_LIMIT);

                        let preloadLength = 0;
                        let preloadPos = 0;
                        let preload: Uint8Array = new Uint8Array(0);
                        const onPreloadMessage = (event: { data: ArrayBuffer }) => {
                            let data = new Uint8Array(event.data);
                            if (preloadLength === 0) {
                                preloadLength = data[0] + (data[1] << 8) + (data[2] << 16) + (data[3] << 24);
                                preload = new Uint8Array(preloadLength - 4);
                                data = data.slice(4);
                            }

                            if (preloadPos < preloadLength) {
                                preload.set(data, preloadPos);
                                preloadPos += data.length;
                            }

                            if (preloadPos > preload.length) {
                                const error = new Error("Preload buffer is bigger then expected");
                                this.errorFn(error);
                                reject(error);
                            } else if (preloadPos == preload.length) {
                                this.preloadQueue = [];
                                if (this.preloadSectors) {
                                    for (let i = 0; i < preload.length; i += 4) {
                                        this.preloadQueue.push(preload[i] + (preload[i + 1] << 8) +
                                            (preload[i + 2] << 16) + (preload[i + 3] << 24));
                                    }
                                }
                                socket.removeEventListener("message", onPreloadMessage);
                                socket.addEventListener("message", onMessage);
                                this.readOnly = mode !== "write";
                                this.openFn(true, !this.readOnly,
                                    (Number.parseInt(sizeStr) ?? 2 * 1024 * 1024) * 1024,
                                    this.preloadQueue,
                                    aheadRange);
                                resolve(socket);

                                if (this.preloadQueue.length > 0) {
                                    if (this.preloadQueue.length * this.aheadSize > MEMORY_LIMIT) {
                                        console.log("WARN! preloadQueue size is bigger then allowed",
                                            this.preloadQueue.length * this.aheadSize / 1024 / 1024, ">",
                                            MEMORY_LIMIT / 1024 / 1024);
                                        this.preloadQueue = this.preloadQueue
                                            .slice(0, Math.floor(MEMORY_LIMIT / this.aheadSize));
                                    }
                                    this.request = this.makeReadRequest(this.preloadQueue.shift()!);
                                    this.executeRequest(this.request);
                                }
                            }
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
                this.retries--;
                if (this.retries === 0) {
                    const error = new Error("Network problem");
                    this.errorFn(error);
                    reject(error);
                } else {
                    setTimeout(this.reconnect.bind(this), 300);
                }
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

    private makeReadRequest(sector: number, buffer: number = -1, resolve: (res: number) => void = () => { }): Request {
        const sectors: number[] = [sector];
        if (this.preloadQueue.length > 0) {
            this.preloadProgressFn(this.preloadQueue.length * this.aheadSize);
            while (this.preloadQueue.length > 0 && sectors.length < this.maxRead) {
                const preload = this.preloadQueue.shift()!;
                if (preload !== sector) {
                    sectors.push(preload);
                }
            }
            for (let i = 0; i < this.preloadQueue.length; ++i) {
                if (this.preloadQueue[i] == sector) {
                    this.preloadQueue.splice(i, 1);
                    break;
                }
            }
        }
        return {
            type: 1,
            sectors,
            buffer,
            resolve,
        };
    }

    public read(sector: number, buffer: Ptr, sync: boolean): Promise<number> | number {
        const cached = this.cache!.read(sector, this.originReadMode);
        if (cached !== null) {
            this.stats.cacheHit++;
            this.module.HEAPU8.set(cached, buffer);
            return sync ? 0 : Promise.resolve(0);
        } else if (sync) {
            return 255;
        } else {
            return new Promise<number>((resolve) => {
                this.stats.cacheMiss++;

                const request: Request = this.makeReadRequest(sector, buffer, resolve);
                if (this.request !== null) {
                    if (this.pendingRequest === null) {
                        this.pendingRequest = request;
                    } else {
                        console.error("New read request while old one is still processed");
                        resolve(3);
                    }
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
            sectors: [sector],
            buffer: this.module.HEAPU8.slice(buffer, buffer + this.sectorSize),
            resolve: () => {/**/},
        };
        this.cache!.write(sector, request.buffer as Uint8Array);
        this.executeRequest(request);
        return 0;
    }

    public async close() {
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
        this.socket.then((socket) => {
            if (request.type === 1) {
                const { sectors } = request;
                this.readStartedAt = Date.now();
                this.readAheadPos = 0;
                this.readAheadCompressed = 0;
                this.readBuffer[0] = 1; // read
                this.readBuffer[1] = sectors.length & 0xFF;
                this.readBuffer[2] = (sectors.length >> 8) & 0xFF;
                this.readBuffer[3] = (sectors.length >> 16) & 0xFF;
                this.readBuffer[4] = (sectors.length >> 24) & 0xFF;

                for (let i = 0; i < sectors.length; ++i) {
                    const origin = this.cache!.getOrigin(sectors[i]);

                    if (i > 0 && origin !== sectors[i]) {
                        console.error("Assertion failed orign should equal to sector", origin, sectors[i]);
                        request.resolve(5);
                        return;
                    }

                    this.readBuffer[5 + i * 4] = origin & 0xFF;
                    this.readBuffer[5 + i * 4 + 1] = (origin >> 8) & 0xFF;
                    this.readBuffer[5 + i * 4 + 2] = (origin >> 16) & 0xFF;
                    this.readBuffer[5 + i * 4 + 3] = (origin >> 24) & 0xFF;
                }
                socket.send(this.readBuffer.slice(0, sectors.length * 4 + 5).buffer);
            } else if (this.readOnly) {
                request.resolve(0);
            } else {
                const { sectors, buffer, resolve } = request;
                this.stats.write += this.sectorSize;
                this.writeBuffer[0] = 2; // write
                this.writeBuffer[1] = sectors[0] & 0xFF;
                this.writeBuffer[2] = (sectors[0] >> 8) & 0xFF;
                this.writeBuffer[3] = (sectors[0] >> 16) & 0xFF;
                this.writeBuffer[4] = (sectors[0] >> 24) & 0xFF;
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
            let data = new Uint8Array(event.data);
            if (this.readAheadCompressed === 0) {
                this.readAheadCompressed = data[0] + (data[1] << 8) + (data[2] << 16) + (data[3] << 24);
                data = data.slice(4);
            }

            const { sectors, buffer, resolve } = this.request;
            const restLength = this.readAheadCompressed - this.readAheadPos;
            if (data.byteLength > restLength || restLength < 0) {
                console.error("wrong read payload length " + data.byteLength + " instead of " + restLength);
                resolve(3);
            } else {
                this.readAheadBuffer.set(data, this.readAheadPos);
                this.readAheadPos += data.byteLength;
                const expectedSize = this.aheadSize * sectors.length;

                if (this.readAheadPos == this.readAheadCompressed) {
                    let decodeResult = expectedSize;
                    if (this.readAheadCompressed < expectedSize) {
                        const result = decodeLz4(this.readAheadBuffer, this.decodeBuffer, 0, this.readAheadCompressed);
                        if (result < 0) {
                            decodeResult = result;
                        } else {
                            this.readAheadBuffer.set(this.decodeBuffer.slice(0, expectedSize), 0);
                        }
                    }

                    if (decodeResult != expectedSize) {
                        console.error("wrong decode result " + decodeResult + " should be " + expectedSize);
                        resolve(4);
                    } else {
                        for (let i = 0; i < sectors.length; ++i) {
                            const aheadOffset = i * this.aheadSize;
                            const sector = sectors[i];
                            const origin = this.cache!.getOrigin(sector);
                            this.cache!.create(origin,
                                this.readAheadBuffer.slice(aheadOffset, aheadOffset + this.aheadSize));
                            if (i == 0 && buffer as number >= 0) {
                                if (this.originReadMode && sector !== origin) {
                                    throw new Error("Sector must be one of origins in originReadMode");
                                }
                                const offset = sector - origin;
                                // aheadOffset is zero
                                const from = offset * this.sectorSize;
                                this.module.HEAPU8.set(this.readAheadBuffer
                                    .slice(from, from + (this.originReadMode ? this.aheadRange : 1) * this.sectorSize),
                                    buffer as number);
                            }
                        }

                        this.stats.cacheUsed = this.cache!.memUsed();
                        this.stats.read += this.readAheadCompressed;
                        this.stats.readTotalTime += Date.now() - this.readStartedAt;
                        this.request = null;
                        resolve(0);

                        if (this.pendingRequest !== null) {
                            this.request = this.pendingRequest;
                            this.pendingRequest = null;
                            this.executeRequest(this.request);
                        } else if (this.preloadQueue.length > 0) {
                            this.request = this.makeReadRequest(this.preloadQueue.shift()!);
                            this.executeRequest(this.request);
                        }
                    }
                }
            }
        } else {
            console.error("Received non arraybuffer data");
            this.reconnect();
        }
    }
}

/**
 * Decode a block. Assumptions: input contains all sequences of a
 * chunk, output is large enough to receive the decoded data.
 * If the output buffer is too small, an error will be thrown.
 * If the returned value is negative, an error occured at the returned offset.
 *
 * @param {ArrayBufferView} input input data
 * @param {ArrayBufferView} output output data
 * @param {number=} sIdx
 * @param {number=} eIdx
 * @return {number} number of decoded bytes
 * @private
 */
function decodeLz4(input: Uint8Array, output: Uint8Array, sIdx: number, eIdx: number) {
    sIdx = sIdx || 0;
    eIdx = eIdx || (input.length - sIdx);
    // Process each sequence in the incoming data
    let i; let n; let j;
    for (i = sIdx, n = eIdx, j = 0; i < n;) {
        const token = input[i++];

        // Literals
        let literalsLength = (token >> 4);
        if (literalsLength > 0) {
            // length of literals
            let l = literalsLength + 240;
            while (l === 255) {
                l = input[i++];
                literalsLength += l;
            }

            // Copy the literals
            const end = i + literalsLength;
            while (i < end) output[j++] = input[i++];

            // End of buffer?
            if (i === n) return j;
        }

        // Match copy
        // 2 bytes offset (little endian)
        const offset = input[i++] | (input[i++] << 8);

        // XXX 0 is an invalid offset value
        if (offset === 0) return j;
        if (offset > j) return -(i - 2);

        // length of match copy
        let matchLength = (token & 0xf);
        let l = matchLength + 240;
        while (l === 255) {
            l = input[i++];
            matchLength += l;
        }

        // Copy the match
        let pos = j - offset; // position of the match copy in the current output
        const end = j + matchLength + 4; // minmatch = 4
        while (j < end) output[j++] = output[pos++];
    }

    return j;
};
