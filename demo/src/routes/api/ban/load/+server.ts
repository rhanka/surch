import { json } from '@sveltejs/kit';
import { getBanTinyLoadPayload } from '$lib/server/ban/bulk';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async () => {
  return json(getBanTinyLoadPayload());
};
