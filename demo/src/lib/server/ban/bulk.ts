import type { BanDocument } from '$lib/types';
import { getBanTinyBulkNdjson } from '../fixture';

export type BanLoadPayload = {
  dataset: string;
  summary: {
    name: string;
    documentCount: number;
    indexName: string;
  };
  bulk: {
    path: '/_bulk';
    contentType: 'application/x-ndjson';
    documentCount: number;
    ndjson: string;
  };
};

export type BanLoadPayloadRequest = {
  datasetName: string;
  indexName: string;
  documents: BanDocument[];
};

export function getBanTinyLoadPayload(): BanLoadPayload {
  const ndjson = getBanTinyBulkNdjson();
  const documentCount = parseBanBulkNdjson(ndjson).length;

  return createBanLoadPayload({
    datasetName: 'ban_tiny',
    indexName: 'ban_tiny',
    documentCount,
    ndjson
  });
}

export function getBanLoadPayload({
  datasetName,
  indexName,
  documents
}: BanLoadPayloadRequest): BanLoadPayload {
  return createBanLoadPayload({
    datasetName,
    indexName,
    documentCount: documents.length,
    ndjson: documentsToBulkNdjson(indexName, documents)
  });
}

function createBanLoadPayload(input: {
  datasetName: string;
  indexName: string;
  documentCount: number;
  ndjson: string;
}): BanLoadPayload {
  return {
    dataset: input.datasetName,
    summary: {
      name: input.datasetName,
      documentCount: input.documentCount,
      indexName: input.indexName
    },
    bulk: {
      path: '/_bulk',
      contentType: 'application/x-ndjson',
      documentCount: input.documentCount,
      ndjson: input.ndjson
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
