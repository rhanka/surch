import { json } from '@sveltejs/kit';
import { parseEngineId, resetBanTiny } from '$lib/server/engines';
import { toDemoErrorResponse } from '$lib/server/demoErrors';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const engine = parseEngineId(body.engine ?? 'surch');

  try {
    return json(await resetBanTiny(engine, fetch));
  } catch (error) {
    const formatted = toDemoErrorResponse(error);

    if (formatted) {
      return json(formatted.body, { status: formatted.status });
    }

    throw error;
  }
};
