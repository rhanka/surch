import { describe, expect, it } from 'vitest';
import type { BanDocument } from '$lib/types';
import { getBanTinyBulkNdjson } from '../fixture';
import {
  documentsToBulkNdjson,
  getBanLoadPayload,
  getBanTinyLoadPayload,
  parseBanBulkNdjson
} from './bulk';

describe('BAN bulk payload', () => {
  const documents: BanDocument[] = [
    {
      city_code: '44109',
      city_name: 'Nantes',
      house_number: '1',
      id: '44109_0001_00001',
      label: '1 Rue de Strasbourg 44000 Nantes',
      location: {
        lat: 47.218371,
        lon: -1.553621
      },
      postcode: '44000',
      source: 'BAN',
      street_name: 'Rue de Strasbourg'
    },
    {
      city_code: '59350',
      city_name: 'Lille',
      house_number: '10',
      id: '59350_0002_00010',
      label: '10 Rue Nationale 59000 Lille',
      location: {
        lat: 50.636565,
        lon: 3.063528
      },
      postcode: '59000',
      source: 'BAN',
      street_name: 'Rue Nationale'
    }
  ];

  it('builds an OpenSearch bulk payload for an arbitrary BAN index', () => {
    const payload = getBanLoadPayload({
      datasetName: 'ban_official_sample',
      indexName: 'ban_official_2026',
      documents
    });

    expect(payload.dataset).toBe('ban_official_sample');
    expect(payload.summary).toEqual({
      name: 'ban_official_sample',
      documentCount: 2,
      indexName: 'ban_official_2026'
    });
    expect(payload.bulk).toMatchObject({
      path: '/_bulk',
      contentType: 'application/x-ndjson',
      documentCount: 2
    });
    expect(payload.bulk.ndjson).toBe(documentsToBulkNdjson('ban_official_2026', documents));

    const lines = payload.bulk.ndjson.trim().split('\n');
    expect(JSON.parse(lines[0])).toEqual({
      index: { _index: 'ban_official_2026', _id: '44109_0001_00001' }
    });
    expect(JSON.parse(lines[1])).toEqual(documents[0]);
    expect(parseBanBulkNdjson(payload.bulk.ndjson)).toEqual(documents);
  });

  it('parses tiny BAN NDJSON action/source pairs', () => {
    const payload = getBanTinyLoadPayload();
    const documents = parseBanBulkNdjson(payload.bulk.ndjson);

    expect(payload.dataset).toBe('ban_tiny');
    expect(payload.bulk.contentType).toBe('application/x-ndjson');
    expect(documents.map((document) => document.id)).toEqual([
      '75101_0001_00001',
      '33063_0002_00010B',
      '67482_0003_00007'
    ]);
  });

  it('keeps the tiny BAN payload compatible with the fixture', () => {
    const payload = getBanTinyLoadPayload();

    expect(payload.dataset).toBe('ban_tiny');
    expect(payload.summary).toEqual({
      name: 'ban_tiny',
      documentCount: 3,
      indexName: 'ban_tiny'
    });
    expect(payload.bulk.path).toBe('/_bulk');
    expect(payload.bulk.contentType).toBe('application/x-ndjson');
    expect(payload.bulk.documentCount).toBe(3);
    expect(payload.bulk.ndjson).toBe(getBanTinyBulkNdjson());
  });
});
