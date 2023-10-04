import { Ptr, Stats } from "./types";
import { BlockCache, Cache } from "./cache";

interface Module {
    HEAPU8: Uint8Array;
    _decode_lz4_block?: (compressedSize: number,
        decodedSize: number, ptr: Ptr) => number;
}

interface Request {
    type: 1 | 2, // read | write
    sector: number,
    buffer: Ptr | Uint8Array,
    resolve: (result: number) => void,
}

export class Drive {
    module: Module;
    sectorSize: number;
    endpoint: string;
    owner: string;
    drive: string;
    token: string;
    stats: Stats;

    socket: Promise<WebSocket>;
    request: Request | null;

    aheadRange: number;
    aheadSize: number;
    writeBuffer: Uint8Array;

    readBuffer: Uint8Array;
    readAheadBuffer: Ptr;
    readAheadPos = 0;
    readAheadCompressed = 0;
    readStartedAt: number;
    decodeBuffer: Uint8Array;

    cache: Cache;
    cleanup = () => {/**/};

    errorFn = (e: Error) => {/**/};

    retries: number;

    public constructor(endpoint: string,
        owner: string,
        drive: string,
        token: string,
        stats: Stats,
        module: Module,
        readAheadBuffer: Ptr,
        aheadRange: number,
        sectorSize = 512,
        memoryLimit = 32 * 1024 * 1024) {
        if (aheadRange > 255) {
            throw new Error("Maximum aheadRange is 255");
        }

        this.sectorSize = sectorSize;
        this.module = module;
        this.endpoint = endpoint;
        this.owner = owner;
        this.drive = drive;
        this.token = token;
        this.request = null;
        this.readBuffer = new Uint8Array(1 + 4 + 1);
        this.writeBuffer = new Uint8Array(1 + 4 + this.sectorSize);
        this.readAheadBuffer = readAheadBuffer;
        this.stats = stats;
        this.aheadRange = aheadRange;
        this.aheadSize = aheadRange * this.sectorSize;
        this.decodeBuffer = new Uint8Array(this.aheadSize);
        this.cache = new BlockCache(this.sectorSize, aheadRange, memoryLimit);

        this.retries = 3;

        this.reconnect();
    }

    public onError(errorFn: (e: Error) => void) {
        this.errorFn = errorFn;
    }

    public reconnect(): void {
        this.socket = new Promise<WebSocket>((resolve, reject) => {
            const socket = new WebSocket(this.endpoint);
            socket.binaryType = "arraybuffer";
            const onMessage = this.onMessage.bind(this);
            const onOpen = () => {
                const onInit = (event: { data: string }) => {
                    socket.removeEventListener("message", onInit);
                    if (event.data === "Ok") {
                        socket.addEventListener("message", onMessage);
                        resolve(socket);
                    } else {
                        const error = new Error("Unable to establish connection");
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

    public read(sector: number, buffer: Ptr, sync: boolean): Promise<number> | number {
        if (this.request !== null && this.request.type === 1) {
            console.error("New read request while old one is still processed");
            return sync ? 3 : Promise.resolve(3);
        }

        const cached = this.cache.read(sector);
        if (cached !== null) {
            if (sync) {
                this.stats.cacheHit++;
            }
            this.request = null;
            this.module.HEAPU8.set(cached, buffer);
            return sync ? 0 : Promise.resolve(0);
        } else if (sync) {
            return 255;
        }

        this.stats.cacheMiss++;

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

    public write(sector: number, buffer: Ptr): number {
        this.request = {
            type: 2,
            sector,
            buffer: this.module.HEAPU8.slice(buffer, buffer + this.sectorSize),
            resolve: () => {/**/},
        };
        this.cache.write(sector, this.request.buffer as Uint8Array);
        this.executeRequest(this.request);

        return 0;
    }

    public async close() {
        try {
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
        } finally {
            this.readAheadBuffer = 0;
        }
    }

    private executeRequest(request: Request) {
        this.socket.then((socket) => {
            if (request.type === 1) {
                const { sector } = request;
                const origin = this.cache.getOrigin(sector);
                this.readStartedAt = Date.now();
                this.readAheadPos = 0;
                this.readAheadCompressed = 0;

                this.readBuffer[0] = 1; // read
                this.readBuffer[1] = origin & 0xFF;
                this.readBuffer[2] = (origin >> 8) & 0xFF;
                this.readBuffer[3] = (origin >> 16) & 0xFF;
                this.readBuffer[4] = (origin >> 24) & 0xFF;
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
            let data = new Uint8Array(event.data);
            if (this.readAheadCompressed === 0) {
                this.readAheadCompressed = data[0] + (data[1] << 8) + (data[2] << 16) + (data[3] << 24);
                data = data.slice(4);
            }

            const { sector, buffer, resolve } = this.request;
            const restLength = this.readAheadCompressed - this.readAheadPos;
            if (data.byteLength > restLength || restLength < 0 || restLength > this.aheadSize) {
                console.error("wrong read payload length " + data.byteLength + " instead of " + restLength);
                resolve(3);
            } else {
                this.module.HEAPU8.set(data, this.readAheadBuffer + this.readAheadPos);
                this.readAheadPos += data.byteLength;

                if (this.readAheadPos == this.readAheadCompressed) {
                    let decodeResult = this.aheadSize;
                    if (this.readAheadCompressed != this.aheadSize) {
                        if (this.module._decode_lz4_block !== undefined) {
                            decodeResult = this.module._decode_lz4_block(
                                this.readAheadCompressed, this.aheadSize, this.readAheadBuffer);
                        } else {
                            const result = decodeLz4(this.module.HEAPU8.slice(this.readAheadBuffer,
                                this.readAheadBuffer + this.aheadSize), this.decodeBuffer, 0, 0);
                            if (result < 0) {
                                decodeResult = result;
                            } else {
                                this.module.HEAPU8.set(this.decodeBuffer, this.readAheadBuffer);
                            }
                        }
                    }

                    if (decodeResult != this.aheadSize) {
                        console.error("wrong decode result " + decodeResult + " should be " + this.aheadSize);
                        resolve(4);
                    } else {
                        const origin = this.cache.getOrigin(sector);
                        this.cache.create(origin,
                            this.module.HEAPU8.slice(this.readAheadBuffer, this.readAheadBuffer + this.aheadSize));
                        this.stats.cacheUsed = this.cache.memUsed();
                        const offset = sector - origin;
                        this.module.HEAPU8.set(
                            this.module.HEAPU8.slice(this.readAheadBuffer + offset * this.sectorSize,
                                this.readAheadBuffer + (offset + 1) * this.sectorSize),
                            buffer as number);
                        this.stats.read += this.readAheadCompressed;
                        this.stats.readTotalTime += Date.now() - this.readStartedAt;
                        this.request = null;
                        resolve(0);
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
