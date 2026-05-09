import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  BAN_ACTIVE_INDEX,
  compareBanIndexSearch,
  compareBanQuery,
  loadBanDocuments,
  resetBanTiny,
  runBanIndexSearch,
  runBanQuery
} from './engines';

describe('engine operations', () => {
  const banDocument = {
    city_code: '75101',
    city_name: 'Paris',
    house_number: '1',
    id: '75101_0001_00001',
    label: '1 Rue de Rivoli 75001 Paris',
    location: { lat: 48.8566, lon: 2.3522 },
    postcode: '75001',
    source: 'BAN' as const,
    street_name: 'Rue de Rivoli'
  };

  afterEach(() => {
    vi.useRealTimers();
  });

  it('resets BAN tiny with fixed OpenSearch-compatible operations', async () => {
    const calls: Array<{ url: string; method: string; body?: string }> = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined
      });

      return Response.json({ acknowledged: true });
    };

    const result = await resetBanTiny('surch', fakeFetch);

    expect(result.engine).toBe('surch');
    expect(result.operations).toEqual([
      { path: '/ban_tiny', status: 200 },
      { path: '/ban_tiny', status: 200 },
      { path: '/_bulk', status: 200 },
      { path: '/ban_tiny/_refresh', status: 200 }
    ]);
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['DELETE', '/ban_tiny'],
      ['PUT', '/ban_tiny'],
      ['POST', '/_bulk'],
      ['POST', '/ban_tiny/_refresh']
    ]);
    expect(calls[2].body).toContain('75101_0001_00001');
  });

  it('tolerates a missing index while resetting BAN tiny', async () => {
    const statuses = [404, 200, 200, 200];
    const fakeFetch = async () => {
      const status = statuses.shift();
      if (!status) {
        throw new Error('unexpected extra request');
      }

      return Response.json({ acknowledged: true }, { status });
    };

    const result = await resetBanTiny('opensearch', fakeFetch);

    expect(result.operations[0]).toEqual({ path: '/ban_tiny', status: 404 });
    expect(result.operations.map((operation) => operation.status)).toEqual([404, 200, 200, 200]);
  });

  it('runs predefined queries without accepting arbitrary paths', async () => {
    const calls: Array<{ url: string; method: string }> = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET'
      });

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '75101_0001_00001' }]
        }
      });
    };

    const result = await runBanQuery({ engine: 'surch', queryId: 'match_label' }, fakeFetch);
    const response = result.response as { hits: { total: { value: number } } };

    expect(result.engine).toBe('surch');
    expect(result.query.id).toBe('match_label');
    expect(response.hits.total.value).toBe(1);
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['POST', '/ban_tiny/_search']
    ]);
  });

  it('compares the same query on Surch and OpenSearch', async () => {
    const seen = new Set<string>();
    const fakeFetch = async (url: URL | RequestInfo) => {
      seen.add(new URL(url.toString()).origin);

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '67482_0003_00007' }]
        }
      });
    };

    const result = await compareBanQuery('fuzzy_label', fakeFetch);

    expect(result.query.id).toBe('fuzzy_label');
    expect(result.surch.status).toBe(200);
    expect(result.opensearch.status).toBe(200);
    expect([...seen].sort()).toEqual(['http://127.0.0.1:7700', 'http://127.0.0.1:9200']);
  });

  it('returns a partial compare result when OpenSearch fails after Surch responds', async () => {
    const fakeFetch = async (url: URL | RequestInfo) => {
      const requestUrl = new URL(url.toString());

      if (requestUrl.origin === 'http://127.0.0.1:9200') {
        throw new Error('connection refused');
      }

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '67482_0003_00007' }]
        }
      });
    };

    const result = await compareBanQuery('fuzzy_label', fakeFetch);

    expect(result.partial).toBe(true);
    expect(result.surch.status).toBe(200);
    expect(result.opensearch).toMatchObject({
      status: 502,
      engine: 'opensearch',
      path: '/ban_tiny/_search',
      message: expect.stringContaining('connection refused')
    });
    expect(result.guardrails.length).toBeGreaterThan(0);
  });

  it('loads BAN documents into the active index with OpenSearch-compatible paths', async () => {
    const calls: Array<{ url: string; method: string; body?: string; contentType?: string }> = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      const headers = init?.headers as Record<string, string> | undefined;
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined,
        contentType: headers?.['content-type']
      });

      return Response.json({ acknowledged: true });
    };

    const result = await loadBanDocuments('surch', BAN_ACTIVE_INDEX, [banDocument], fakeFetch);

    expect(result).toEqual({
      engine: 'surch',
      index: 'ban_addresses',
      operations: [
        { path: '/ban_addresses', status: 200 },
        { path: '/ban_addresses', status: 200 },
        { path: '/_bulk', status: 200 },
        { path: '/ban_addresses/_refresh', status: 200 }
      ]
    });
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['DELETE', '/ban_addresses'],
      ['PUT', '/ban_addresses'],
      ['POST', '/_bulk'],
      ['POST', '/ban_addresses/_refresh']
    ]);
    expect(calls[2].body).toBe(
      '{"index":{"_index":"ban_addresses","_id":"75101_0001_00001"}}\n' +
        '{"city_code":"75101","city_name":"Paris","house_number":"1","id":"75101_0001_00001","label":"1 Rue de Rivoli 75001 Paris","location":{"lat":48.8566,"lon":2.3522},"postcode":"75001","source":"BAN","street_name":"Rue de Rivoli"}\n'
    );
    expect(calls[2].contentType).toBe('application/x-ndjson');
  });

  it('chunks large BAN loads into multiple bulk requests', async () => {
    const calls: Array<{ path: string; method: string; body?: string }> = [];
    const documents = Array.from({ length: 1001 }, (_, index) => ({
      ...banDocument,
      id: `75101_0001_${String(index).padStart(5, '0')}`,
      house_number: String(index)
    }));
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        path: new URL(url.toString()).pathname,
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined
      });

      return Response.json({ acknowledged: true });
    };

    const result = await loadBanDocuments('surch', BAN_ACTIVE_INDEX, documents, fakeFetch);
    const bulkCalls = calls.filter((call) => call.path === '/_bulk');

    expect(result.operations.filter((operation) => operation.path === '/_bulk')).toHaveLength(2);
    expect(bulkCalls).toHaveLength(2);
    expect(bulkCalls[0].body?.includes('75101_0001_00000')).toBe(true);
    expect(bulkCalls[1].body?.includes('75101_0001_01000')).toBe(true);
  });

  it('allows active BAN lifecycle operations to outlive the generic demo timeout', async () => {
    vi.useFakeTimers();
    const calls: string[] = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push(new URL(url.toString()).pathname);

      return new Promise<Response>((resolve, reject) => {
        const signal = init?.signal as AbortSignal | undefined;
        signal?.addEventListener('abort', () => {
          reject(new DOMException('aborted', 'AbortError'));
        });
        setTimeout(() => resolve(Response.json({ acknowledged: true })), 3_000);
      });
    };

    const load = loadBanDocuments('surch', BAN_ACTIVE_INDEX, [banDocument], fakeFetch);
    for (let index = 0; index < 4; index += 1) {
      await vi.advanceTimersByTimeAsync(3_000);
    }

    await expect(load).resolves.toMatchObject({
      engine: 'surch',
      index: BAN_ACTIVE_INDEX
    });
    expect(calls).toEqual(['/ban_addresses', '/ban_addresses', '/_bulk', '/ban_addresses/_refresh']);
  });

  it('runs a controlled free search on the active BAN index', async () => {
    const calls: Array<{ url: string; method: string; body?: string }> = [];
    const searchBody = { query: { match: { label: 'rivoli' } }, size: 5 };
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined
      });

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '75101_0001_00001' }]
        }
      });
    };

    const result = await runBanIndexSearch(
      { engine: 'opensearch', index: BAN_ACTIVE_INDEX, body: searchBody },
      fakeFetch
    );

    expect(result.engine).toBe('opensearch');
    expect(result.index).toBe('ban_addresses');
    expect(result.response).toMatchObject({ hits: { total: { value: 1 } } });
    expect(calls.map((call) => [call.method, new URL(call.url).pathname, call.body])).toEqual([
      ['POST', '/ban_addresses/_search', JSON.stringify(searchBody)]
    ]);
  });

  it('compares the same free BAN index search on Surch and OpenSearch', async () => {
    const calls: Array<{ origin: string; path: string; body?: string }> = [];
    const searchBody = { query: { term: { postcode: '75001' } } };
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      const requestUrl = new URL(url.toString());
      calls.push({
        origin: requestUrl.origin,
        path: requestUrl.pathname,
        body: typeof init?.body === 'string' ? init.body : undefined
      });

      return Response.json({ hits: { total: { value: 1, relation: 'eq' }, hits: [] } });
    };

    const result = await compareBanIndexSearch(BAN_ACTIVE_INDEX, searchBody, fakeFetch);

    expect(result.index).toBe('ban_addresses');
    expect(result.partial).toBe(false);
    expect(result.surch.status).toBe(200);
    expect(result.opensearch.status).toBe(200);
    expect(calls).toEqual(
      expect.arrayContaining([
        {
          origin: 'http://127.0.0.1:7700',
          path: '/ban_addresses/_search',
          body: JSON.stringify(searchBody)
        },
        {
          origin: 'http://127.0.0.1:9200',
          path: '/ban_addresses/_search',
          body: JSON.stringify(searchBody)
        }
      ])
    );
  });

  it('rejects invalid index paths before contacting an engine', async () => {
    const fakeFetch = async () => {
      throw new Error('fetch should not be called');
    };

    await expect(loadBanDocuments('surch', '../ban_addresses', [banDocument], fakeFetch)).rejects.toThrow(
      'engine path is outside the fixed BAN demo surface'
    );
    await expect(
      runBanIndexSearch({ engine: 'surch', index: 'ban_addresses/_cat', body: { query: { match_all: {} } } }, fakeFetch)
    ).rejects.toThrow('engine path is outside the fixed BAN demo surface');
  });
});
