import type { LayoutServerLoad } from './$types';
import { getClient } from '$lib/server/grpc.js';

export const load: LayoutServerLoad = async () => {
	try {
		// Quick connectivity check — list with no filter
		await getClient().list({});
		return { daemonConnected: true };
	} catch {
		return { daemonConnected: false };
	}
};
