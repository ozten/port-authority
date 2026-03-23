import { json, type RequestHandler } from '@sveltejs/kit';
import { getClient } from '$lib/server/grpc.js';
import { toReservationViewModel, serializeReservation } from '$lib/mappers/reservation.js';
import { grpcErrorToMessage } from '$lib/utils/errors.js';

// GET — List reservations
export const GET: RequestHandler = async ({ url }) => {
	try {
		const ownerFilter = url.searchParams.get('owner') ?? undefined;
		const stateParam = url.searchParams.get('state');
		const stateFilter = stateParam ? parseInt(stateParam, 10) : undefined;

		const response = await getClient().list({
			ownerFilter,
			stateFilter: stateFilter !== undefined && !isNaN(stateFilter) ? stateFilter : undefined
		});

		const reservations = (response.reservations || []).map((r) =>
			serializeReservation(toReservationViewModel(r))
		);

		return json({ reservations });
	} catch (err) {
		return json({ reservations: [], error: grpcErrorToMessage(err) }, { status: 502 });
	}
};

// POST — Reserve a port
export const POST: RequestHandler = async ({ request }) => {
	try {
		const body = await request.json();

		const response = await getClient().reserve({
			owner: body.owner,
			preferredPort: body.preferred_port ?? undefined,
			targetHost: body.target_host,
			targetPort: body.target_port,
			leaseSeconds: body.lease_seconds ?? undefined,
			exactOnly: body.exact_only ?? false
		});

		return json({
			reservation_id: response.reservationId,
			assigned_port: response.assignedPort,
			state: response.state
		});
	} catch (err) {
		return json({ error: grpcErrorToMessage(err) }, { status: 400 });
	}
};

// DELETE — Release a reservation
export const DELETE: RequestHandler = async ({ request }) => {
	try {
		const body = await request.json();

		const identifier = body.reservation_id
			? { $case: 'reservationId' as const, reservationId: body.reservation_id as string }
			: body.port
				? { $case: 'port' as const, port: body.port as number }
				: undefined;

		if (!identifier) {
			return json({ error: 'Must provide reservation_id or port' }, { status: 400 });
		}

		const response = await getClient().release({ identifier });
		return json({ success: response.success, message: response.message });
	} catch (err) {
		return json({ error: grpcErrorToMessage(err) }, { status: 400 });
	}
};
