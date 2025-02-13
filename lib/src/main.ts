enum OpResult {
	OK = 0,
	AGAIN = 255,
}

interface OpReadResult {
	result: OpResult;
	data: Uint8Array;
}

export interface Drive {
	read(sector: number, sync: boolean): Promise<OpReadResult> | OpReadResult;
	write(sector: number, data: Uint8Array): OpResult;
}

export default function sockdrive(url: string): Drive {
	return {};
}