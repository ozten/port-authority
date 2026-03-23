import type { PageServerLoad } from './$types';
import { getClient } from '$lib/server/grpc.js';
import { toReservationViewModel, serializeReservation } from '$lib/mappers/reservation.js';

export const load: PageServerLoad = async () => {
	try {
		const response = await getClient().list({});
		const reservations = (response.reservations || []).map((r) => {
			const vm = toReservationViewModel(r);
			return serializeReservation(vm);
		});
		return { reservations, error: null };
	} catch (err) {
		return {
			reservations: [],
			error: err instanceof Error ? err.message : 'Failed to load reservations'
		};
	}
};
