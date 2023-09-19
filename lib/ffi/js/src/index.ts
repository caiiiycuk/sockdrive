import { createFileSystem } from "./fatfs/index";

(window as any).createFileSystem = createFileSystem;
