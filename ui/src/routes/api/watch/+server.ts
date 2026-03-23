import type { RequestHandler } from '@sveltejs/kit';
import { getWatchBridge } from '$lib/server/watch-bridge.js';

export const GET: RequestHandler = async ({ request }) => {
	const bridge = getWatchBridge();
	const encoder = new TextEncoder();

	const stream = new ReadableStream({
		start(controller) {
			const send = (data: Record<string, unknown>) => {
				try {
					controller.enqueue(encoder.encode(`data: ${JSON.stringify(data)}\n\n`));
				} catch {
					// Controller may be closed
				}
			};

			const sendRefresh = () => {
				try {
					controller.enqueue(encoder.encode(`event: refresh\ndata: {}\n\n`));
				} catch {
					// Controller may be closed
				}
			};

			// Keepalive every 15 seconds
			const keepalive = setInterval(() => {
				try {
					controller.enqueue(encoder.encode(`: keepalive\n\n`));
				} catch {
					clearInterval(keepalive);
				}
			}, 15_000);

			let unsubscribe: (() => void) | null = null;
			try {
				unsubscribe = bridge.subscribe(send, sendRefresh);
			} catch {
				// Too many connections
				controller.enqueue(
					encoder.encode(`event: error\ndata: {"message":"Too many connections"}\n\n`)
				);
				clearInterval(keepalive);
				controller.close();
				return;
			}

			// Cleanup on client disconnect
			request.signal.addEventListener('abort', () => {
				clearInterval(keepalive);
				unsubscribe?.();
				try {
					controller.close();
				} catch {
					// Already closed
				}
			});
		}
	});

	return new Response(stream, {
		headers: {
			'Content-Type': 'text/event-stream',
			'Cache-Control': 'no-cache',
			Connection: 'keep-alive'
		}
	});
};
