import type { Handle } from '@sveltejs/kit';

export const handle: Handle = async ({ event, resolve }) => {
	// Origin validation: block cross-origin requests to mutation endpoints
	const origin = event.request.headers.get('origin');
	if (origin) {
		try {
			const url = new URL(origin);
			const isLocal =
				url.hostname === 'localhost' ||
				url.hostname === '127.0.0.1' ||
				url.hostname === '::1';
			if (!isLocal) {
				return new Response('Forbidden: cross-origin request', { status: 403 });
			}
		} catch {
			return new Response('Forbidden: invalid origin', { status: 403 });
		}
	}

	return resolve(event);
};
