import { json } from '@sveltejs/kit';
import { parseEngineId, resetBanTiny } from '$lib/server/engines';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const engine = parseEngineId(body.engine ?? 'surch');

  return json(await resetBanTiny(engine, fetch));
};
