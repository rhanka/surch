import { json } from '@sveltejs/kit';
import { getBanTinyFixture } from '$lib/server/fixture';
import type { RequestHandler } from './$types';

export const GET = (() => {
  return json(getBanTinyFixture());
}) satisfies RequestHandler;
