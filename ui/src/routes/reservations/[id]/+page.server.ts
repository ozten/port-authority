import type { PageServerLoad } from './$types';
import { error } from '@sveltejs/kit';
import { getClient } from '$lib/server/grpc.js';
import { toReservationViewModel, serializeReservation } from '$lib/mappers/reservation.js';

export const load: PageServerLoad = async ({ params }) => {
	try {
		const response = await getClient().inspect({
			identifier: {
				$case: 'reservationId',
				reservationId: params.id
			}
		});

		if (!response.reservation) {
			error(404, 'Reservation not found');
		}

		const reservation = serializeReservation(toReservationViewModel(response.reservation));

		const tunnelHealth = response.tunnelHealth
			? {
					alive: response.tunnelHealth.alive,
					lastCheck: response.tunnelHealth.lastCheck
						? new Date(
								Number(response.tunnelHealth.lastCheck.seconds?.low ?? 0) * 1000
							).toISOString()
						: null,
					uptimeSeconds: response.tunnelHealth.uptimeSeconds,
					reconnectCount: response.tunnelHealth.reconnectCount
				}
			: null;

		return { reservation, tunnelHealth };
	} catch (err) {
		if (err && typeof err === 'object' && 'status' in err) throw err;
		error(502, err instanceof Error ? err.message : 'Failed to load reservation');
	}
};
