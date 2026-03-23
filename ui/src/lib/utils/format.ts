const rtf = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
	['second', 60],
	['minute', 60],
	['hour', 24],
	['day', 30],
	['month', 12],
	['year', Infinity]
];

export function relativeTime(date: Date): string {
	let seconds = Math.round((date.getTime() - Date.now()) / 1000);
	for (const [unit, max] of UNITS) {
		if (Math.abs(seconds) < max) {
			return rtf.format(Math.round(seconds), unit);
		}
		seconds /= max;
	}
	return date.toLocaleDateString();
}

export function absoluteTime(date: Date): string {
	return date.toLocaleString();
}

export function formatDuration(ms: number): string {
	if (ms <= 0) return '0s';
	const totalSeconds = Math.floor(ms / 1000);
	const hours = Math.floor(totalSeconds / 3600);
	const minutes = Math.floor((totalSeconds % 3600) / 60);
	const seconds = totalSeconds % 60;

	const parts: string[] = [];
	if (hours > 0) parts.push(`${hours}h`);
	if (minutes > 0) parts.push(`${minutes}m`);
	if (seconds > 0 || parts.length === 0) parts.push(`${seconds}s`);
	return parts.join(' ');
}

export function formatPort(port: number): string {
	return port.toString();
}

export function formatTarget(host: string, port: number): string {
	return `${host}:${port}`;
}
