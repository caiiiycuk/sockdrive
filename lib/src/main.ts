import ms from 'ms';
import lunchtime from './lunchtime.js';
import millisecondsUntil from './millisecondsUntil.js';

enum SockdriveResult {
	OK = 0,
	AGAIN = 255,
}

export default function howLongUntilLunch(hours: number = 12, minutes: number = 30): string {
	const millisecondsUntilLunchTime = millisecondsUntil(lunchtime(hours, minutes));
	return ms(millisecondsUntilLunchTime, { long: true });
}