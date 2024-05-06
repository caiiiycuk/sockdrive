import { Handle, Ptr, Stats } from "./sockdrive/types";
import { Drive } from "./sockdrive/drive";

interface EmModule {
    HEAPU8: Uint8Array,
    _malloc: (len: number) => Ptr,
    _free: (ptr: Ptr) => void,
};

interface Template {
    name: string,
    size: number,
    heads: number,
    cylinders: number,
    sectors: number,
    sector_size: number,
}

declare const Module: EmModule & any;

(function() {
    let seq = 0;
    const mapping: { [handle: Handle]: Drive } = {};
    const templates: { [handle: number]: Template } = {};
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
        onError: (e: Error) => {
            console.error(e);
        },
        onOpen: (drive: string, read: boolean, write: boolean) => {
            // noop
        },
        open: async (url: string, owner: string, name: string, token: string): Promise<Handle> => {
            const response = await fetch(url.replace("wss://", "https://")
                .replace("ws://", "http://") + "/template/" + owner + "/" + name);
            const template = await response.json();
            if (template.error) {
                throw new Error(template.error);
            }
            seq++;
            templates[seq] = template;
            mapping[seq] = new Drive(url, owner, name, token, stats, Module,
                Module._malloc(512 * 255), 255, 512);
            mapping[seq].onOpen((read, write) => {
                Module.sockdrive.onOpen(owner + "/" + name, read, write);
            });
            mapping[seq].onError(Module.sockdrive.onError);
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
            if (mapping[handle]) {
                return mapping[handle].write(sector, buffer);
            }
            console.error("ERROR! sockdrive handle", handle, "not found");
            return 1;
        },
        close: (handle: Handle) => {
            if (mapping[handle]) {
                Module._free(mapping[handle].readAheadBuffer);
                mapping[handle].close();
                delete mapping[handle];
            }
        },
        size: (handle: Handle) => {
            return templates[handle]?.size ?? 0;
        },
        sector_size: (handle: Handle) => {
            return templates[handle]?.sector_size ?? 512;
        },
        heads: (handle: Handle) => {
            return templates[handle]?.heads ?? 1;
        },
        cylinders: (handle: Handle) => {
            return templates[handle]?.cylinders ?? 520;
        },
        sectors: (handle: Handle) => {
            return templates[handle]?.sectors ?? 63;
        },
    };
})();
