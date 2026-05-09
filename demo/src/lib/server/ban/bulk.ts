import type { BanDocument } from '$lib/types';
import { getBanTinyBulkNdjson } from '../fixture';

export type BanLoadPayload = {
  dataset: string;
  bulk: {
    path: '/_bulk';
    contentType: 'application/x-ndjson';
    documentCount: number;
    ndjson: string;
  };
};

export function getBanTinyLoadPayload(): BanLoadPayload {
  const ndjson = getBanTinyBulkNdjson();

  return {
    dataset: 'ban_tiny',
    bulk: {
      path: '/_bulk',
      contentType: 'application/x-ndjson',
      documentCount: parseBanBulkNdjson(ndjson).length,
      ndjson
    }
  };
}

export function documentsToBulkNdjson(index: string, documents: BanDocument[]): string {
  return `${documents
    .map((document) =>
      [
        JSON.stringify({ index: { _index: index, _id: document.id } }),
        JSON.stringify(document)
      ].join('\n')
    )
    .join('\n')}\n`;
}

export function parseBanBulkNdjson(ndjson: string): BanDocument[] {
  const lines = ndjson.trim().split('\n');
  const documents: BanDocument[] = [];

  for (let index = 0; index < lines.length; index += 2) {
    const source = lines[index + 1];
    if (!source) {
      continue;
    }

    documents.push(JSON.parse(source) as BanDocument);
  }

  return documents;
}
