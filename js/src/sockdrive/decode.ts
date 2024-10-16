export const MAX_FRAME_SIZE = 1 * 1024 * 1024;

export interface Frame {
    sectors: number[],
    sectorsRow: Uint8Array,

    aheadSize: number,
    payload: Uint8Array,
    payloadSize: number,
    payloadPos: number,
    error?: string,
}

export function decodeFrame(ev: { data: ArrayBuffer }, frame: Frame): boolean {
    if (ev.data instanceof ArrayBuffer) {
        let data = new Uint8Array(ev.data);
        if (frame.payloadSize === 0) {
            frame.payloadSize = data[0] + (data[1] << 8) + (data[2] << 16) + (data[3] << 24);
            data = data.slice(4);
        }

        const restLength = frame.payloadSize - frame.payloadPos;
        if (data.byteLength > restLength || restLength < 0) {
            frame.error = "wrong read payload length " + data.byteLength + " instead of " + restLength;
            return true;
        } else {
            frame.payload.set(data, frame.payloadPos);
            frame.payloadPos += data.byteLength;

            if (frame.payloadPos == frame.payloadSize) {
                const expectedSize = frame.aheadSize * frame.sectors.length;
                let decodeResult = expectedSize;
                if (frame.payloadSize < expectedSize) {
                    decodeResult = decodeLz4(frame.payload, frame.sectorsRow, 0, frame.payloadSize);
                } else {
                    frame.sectorsRow.set(frame.payload);
                }

                if (decodeResult != expectedSize) {
                    frame.error = "wrong decode result " + decodeResult + " should be " + expectedSize;
                }
                return true;
            } else {
                return false;
            }
        }
    } else {
        frame.error = "received non arraybuffer data";
        return true;
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
            while (i < end) {
                output[j++] = input[i++];
            }

            // End of buffer?
            if (i === n) {
                return j;
            }
        }

        // Match copy
        // 2 bytes offset (little endian)
        const offset = input[i++] | (input[i++] << 8);

        // XXX 0 is an invalid offset value
        if (offset === 0) {
            return j;
        }

        if (offset > j) {
            return -(i - 2);
        }

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
        while (j < end) {
            output[j++] = output[pos++];
        }
    }

    return j;
};
