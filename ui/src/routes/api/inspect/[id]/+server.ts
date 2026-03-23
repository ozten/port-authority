import { json, type RequestHandler } from '@sveltejs/kit';
import { getClient } from '$lib/server/grpc.js';
import { toReservationViewModel, toTunnelHealthViewModel, serializeReservation } from '$lib/mappers/reservation.js';
import { grpcErrorToMessage } from '$lib/utils/errors.js';

export const GET: RequestHandler = async ({ params }) => {
	try {
		const response = await getClient().inspect({
			identifier: {
				$case: 'reservationId',
				reservationId: params.id!
			}
		});

		const reservation = response.reservation
			? serializeReservation(toReservationViewModel(response.reservation))
			: null;

		const tunnelHealth = response.tunnelHealth
			? {
					alive: response.tunnelHealth.alive,
					lastCheck: response.tunnelHealth.lastCheck
						? new Date(
								Number(response.tunnelHealth.lastCheck.seconds.low) * 1000
							).toISOString()
						: null,
					uptimeSeconds: response.tunnelHealth.uptimeSeconds,
					reconnectCount: response.tunnelHealth.reconnectCount
				}
			: null;

		return json({ reservation, tunnelHealth });
	} catch (err) {
		return json({ error: grpcErrorToMessage(err) }, { status: 400 });
	}
};
