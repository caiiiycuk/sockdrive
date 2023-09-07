import { Handle, Ptr, Stats } from "./types";
import { Drive } from "./drive";

declare const Module: any;

(function () {
    let seq = 0;
    const mapping: { [handle: Handle]: Drive } = {};
    const stats: Stats = {
        read: 0,
        write: 0,
        readTotalTime: 0,
        cacheHit: 0,
        cacheMiss: 0,
        cacheUsed: 0,
    };
    Module.sockdrive = {
        stats,
        open: (host: string, port: number): Handle => {
            seq++;
            mapping[seq] = new Drive("ws://" + host + ":" + port, stats);
            return seq;
        },

        read: (handle: Handle, sector: number, buffer: Ptr, sync: boolean): Promise<number> | number => {
            if (mapping[handle]) {
                return mapping[handle].read(sector, buffer, sync);
            }

            console.error("ERROR! sockdrive handle", handle, "not found");
            return sync ? 1 : Promise.resolve(1);
        },

        write: (handle: Handle, sector: number, buffer: Ptr): number => {
            if  (mapping[handle]) {
                return mapping[handle].write(sector, buffer);
            }
            console.error("ERROR! sockdrive handle", handle, "not found");
            return 1;
        },

        close: (handle: Handle) => {
            if (mapping[handle]) {
                mapping[handle].close();
                delete mapping[handle];
            }
        },
    };
})();