import { EmModule, Handle, Ptr, Stats } from "./sockdrive/types";
import { Drive } from "./sockdrive/drive";
import { Cache } from "./sockdrive/cache";

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
        io: [],
    };
    const cache: { [backend: string]: Cache } = {};
    Module.sockdrive = {
        stats,
        cache,
        createCache: (url: string, preload: boolean) => {
            return new Cache(url, preload);
        },
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
            stats.io.push({ read: 0, write: 0 });
            return new Promise<Handle>((resolve, reject) => {
                const backendCache = cache[url] ?? null;
                if (backendCache) {
                    backendCache.open(owner, name, token);
                } else {
                    console.error("sockdrive cache not found for", url);
                }
                mapping[seq] = new Drive(url, owner, name, token, stats, Module, backendCache);
                mapping[seq].onOpen((read, write, imageSize, preloadQueue, realOwner, realName) => {
                    Module.sockdrive.onOpen((realOwner ?? owner) + "/" + (realName ?? name),
                        read, write, imageSize, preloadQueue);
                    resolve(seq);
                });
                mapping[seq].onError((e) => {
                    Module.sockdrive.onError(e);
                    reject(e);
                });
            });
        },
        read: (handle: Handle, sector: number, buffer: Ptr, sync: boolean): Promise<number> | number => {
            if (mapping[handle]) {
                stats.io[handle - 1].read += 1;
                return mapping[handle].read(sector, buffer, sync);
            }

            console.error("ERROR! sockdrive handle", handle, "not found");
            return sync ? 1 : Promise.resolve(1);
        },
        write: (handle: Handle, sector: number, buffer: Ptr): number => {
            if (mapping[handle]) {
                stats.io[handle - 1].write += 1;
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
        bufferedAmount: () => {
            let total = 0;
            for (const next of Object.values(mapping)) {
                total += next.bufferedAmount();
            }
            return total;
        },
    };
})();
