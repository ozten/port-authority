import { getClient } from './grpc.js';
import { isConnectionError } from '$lib/utils/errors.js';

type EventCallback = (data: Record<string, unknown>) => void;
type RefreshCallback = () => void;

interface Subscriber {
	onEvent: EventCallback;
	onRefresh: RefreshCallback;
}

interface WatchBridge {
	subscribe(onEvent: EventCallback, onRefresh: RefreshCallback): () => void;
	connectionCount(): number;
}

const BRIDGE_KEY = Symbol.for('portd.watch-bridge');
const MAX_SSE_CONNECTIONS = 10;

const STATE_NAMES: Record<number, string> = {
	0: 'unspecified',
	1: 'pending',
	2: 'active',
	3: 'failed',
	4: 'released'
};

function createWatchBridge(): WatchBridge {
	const subscribers = new Set<Subscriber>();
	let watchActive = false;
	let reconnectTimeout: ReturnType<typeof setTimeout> | null = null;
	let abortController: AbortController | null = null;

	async function startWatch(): Promise<void> {
		if (watchActive) return;
		watchActive = true;

		let backoff = 1000;

		while (watchActive) {
			try {
				abortController = new AbortController();
				const client = getClient();
				const stream = client.watch({}, { signal: abortController.signal });

				backoff = 1000; // Reset on successful connection

				for await (const event of stream) {
					const payload = {
						reservationId: event.reservationId,
						oldState: STATE_NAMES[event.oldState] ?? 'unknown',
						newState: STATE_NAMES[event.newState] ?? 'unknown',
						timestamp: event.timestamp
							? new Date(
									Number(event.timestamp.seconds) * 1000 +
										event.timestamp.nanos / 1_000_000
								).toISOString()
							: null,
						message: event.message ?? null
					};
					for (const sub of subscribers) {
						try {
							sub.onEvent(payload);
						} catch {
							// Don't let one subscriber crash the bridge
						}
					}
				}
			} catch (err) {
				if (!watchActive) break;

				// Tell all subscribers to refresh their state
				for (const sub of subscribers) {
					try {
						sub.onRefresh();
					} catch {
						// Ignore
					}
				}

				if (isConnectionError(err)) {
					// Daemon is down, back off
					await new Promise((resolve) => {
						reconnectTimeout = setTimeout(resolve, backoff);
					});
					backoff = Math.min(backoff * 2, 30_000);
				} else {
					// Unexpected error, shorter retry
					await new Promise((resolve) => {
						reconnectTimeout = setTimeout(resolve, 1000);
					});
				}
			}
		}
	}

	function stopWatch(): void {
		watchActive = false;
		abortController?.abort();
		if (reconnectTimeout) {
			clearTimeout(reconnectTimeout);
			reconnectTimeout = null;
		}
	}

	return {
		subscribe(onEvent: EventCallback, onRefresh: RefreshCallback): () => void {
			if (subscribers.size >= MAX_SSE_CONNECTIONS) {
				throw new Error('Too many SSE connections');
			}

			const sub: Subscriber = { onEvent, onRefresh };
			subscribers.add(sub);

			// Start watching if this is the first subscriber
			if (subscribers.size === 1) {
				startWatch();
			}

			return () => {
				subscribers.delete(sub);
				if (subscribers.size === 0) {
					stopWatch();
				}
			};
		},

		connectionCount(): number {
			return subscribers.size;
		}
	};
}

export function getWatchBridge(): WatchBridge {
	const existing = (globalThis as Record<symbol, WatchBridge | undefined>)[BRIDGE_KEY];
	if (existing) return existing;

	const bridge = createWatchBridge();
	(globalThis as Record<symbol, WatchBridge>)[BRIDGE_KEY] = bridge;
	return bridge;
}
