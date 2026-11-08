// Generated from packages/api/openapi.json by `pnpm api:gen`. Don't edit it.
export interface paths {
    "/api/v1/caches": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["CacheStatsDto"];
                    };
                };
                401: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/health": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["HealthDto"];
                    };
                };
                503: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["HealthDto"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/principals": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PrincipalPage"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NewPrincipalBody"];
                };
            };
            responses: {
                201: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PrincipalDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/principals/{id}/disable": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                204: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/session": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["LoginBody"];
                };
            };
            responses: {
                /** @description Signed in. The session is in an HttpOnly cookie. */
                201: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["WhoAmI"];
                    };
                };
                401: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                /** @description The x-memex-csrf header is missing */
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                /** @description Too many failed attempts */
                429: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description Signed out, or there was no session */
                204: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                /** @description The x-memex-csrf header is missing */
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tokens": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                    /** @description Every principal's tokens, for server admins. The default is your own. */
                    all?: boolean;
                };
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["TokenPage"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NewTokenBody"];
                };
            };
            responses: {
                201: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["CreatedTokenDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tokens/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                204: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["VaultPage"];
                    };
                };
            };
        };
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NameBody"];
                };
            };
            responses: {
                201: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["VaultDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["VaultDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                204: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        options?: never;
        head?: never;
        patch: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NameBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["VaultDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        trace?: never;
    };
    "/api/v1/vaults/{id}/activity": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Follows everything happening in a vault as server-sent events. Frames are named `hello`, `durable`, `progress`, `resync`, `heartbeat` and `end`. `durable` and `resync` frames have ids, and a stream resumes after one with `Last-Event-ID` or `after`. A `resync` frame means fetch a new snapshot from `/activity/documents` and carry on. Frames hold ids, kinds, stages, counts and timings, and never document text. */
        get: {
            parameters: {
                query?: {
                    /** @description A position to carry on from, such as a snapshot's `head`, for clients that can't set `Last-Event-ID`. That header comes first when both are there. */
                    after?: string | null;
                };
                header?: {
                    /** @description The last frame id seen. A position that can't be resumed from gives a `resync` frame, not an error. */
                    "Last-Event-ID"?: string | null;
                };
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description A stream of frames, each with the schema of its name: `DurableFrame`, `ProgressFrame`, `ResyncFrame`, `HelloFrame`, `HeartbeatFrame` or `EndFrame` */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "text/event-stream": string;
                    };
                };
                401: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                /** @description This principal has as many streams open as it may, with the code `too_many_streams` */
                429: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/activity/documents": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The live board: each document's stage and status, in id order. `head` is the log position this reflects, to follow the stream from. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string | null;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number | null;
                    /** @description `created`, the default. */
                    sort?: string | null;
                    /** @description Only documents in this stage. */
                    stage?: string | null;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ActivityDocumentPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                401: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/assertions/as-of": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    subject: string;
                    /** @description The sequence number to read as of. */
                    seq: number;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AssertionList"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/assertions/{aid}/evidence/{eid}/highlight": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    aid: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["HighlightDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/assertions/{aid}/evidence/{eid}/highlight.png": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description One of the allowed values. The default is 150. */
                    dpi?: number;
                };
                header?: never;
                path: {
                    id: string;
                    aid: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description A PNG of the page with the region highlighted */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "image/png": unknown;
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/assertions/{aid}/lineage": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    aid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["LineageDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/budget": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditBudgetDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/planted": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Corrupts claims in a test vault and tags them, tagging what's left clean, to measure what audits catch. It's refused on any other vault. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["PlantBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PlantedDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/recall": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** What planted errors came to, per kind and in two groups: kinds the deterministic checks catch, and kinds only LLM checks can. Scripted answers say nothing about a model, so `target_met` stays null until a real model's run. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["RecallReportDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/runs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Runs in creation order, with their seeds, what each found and what it spent. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditRunPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        /** Starts a run now. A seed makes it repeatable: the same seed on the same state samples the same claims as any other run with that seed, scheduled or not. It may spend tokens, so it needs admin scope. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["TriggerAuditBody"];
                };
            };
            responses: {
                202: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditRunDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/runs/{run}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** One run, with its seed and its sample in rank order. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    run: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditRunDetailDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/runs/{run}/checks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** What was found about each sampled claim, in rank order, with each check's result and the review item it led to. `findings=true` leaves out what passed. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                    /** @description Only what didn't pass. */
                    findings?: boolean;
                };
                header?: never;
                path: {
                    id: string;
                    run: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditCheckPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/schedule": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditScheduleDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        /** Replaces the schedule, which is off until someone turns it on. The first run comes one interval from now. */
        put: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["AuditScheduleBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AuditScheduleDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/audit/test-vault": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Marks an empty vault as one errors can be planted in. It can't be undone. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                204: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changes": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EventPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changesets": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                    /** @description `pending`, `committed` or `rejected`. `pending` is the review queue. */
                    state?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChangeSetPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changesets/{cs}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChangeSetDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changesets/{cs}/approve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ReasonBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChangeSetDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changesets/{cs}/inverse": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["InversePreview"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changesets/{cs}/reject": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ReasonBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChangeSetDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/changesets/{cs}/revert": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ReasonBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChangeSetDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/chat/runs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Runs the agent on one message, as AG-UI events over server-sent events. Anything that refuses it comes back as an ordinary error before the stream opens. A failure afterwards is a `RUN_ERROR` with a code, and a budget, a cancel or a reply cut at its limit is a `RUN_FINISHED` whose `result.stop` says so. Disconnecting cancels the run, and it can't be resumed. Needs read scope, and the agent can propose only with propose scope. A thread is the caller's own, and its id is theirs to choose, so a new id starts a new thread. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["RunAgentBody"];
                };
            };
            responses: {
                /** @description AG-UI events */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "text/event-stream": string;
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                429: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                503: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/chat/settings": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Whether chat is on, what it may spend and what it has spent today. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChatSettingsDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        /** Turns chat on or off. It's off until an admin turns it on, because on means this vault's content goes to the configured LLM. Needs admin scope. */
        put: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ChatSettingsBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChatSettingsDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/chat/threads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Your own threads, the most recently updated first, so the only `sort` here is `updated`. Nobody can list another's, whatever their scope. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChatThreadPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/chat/threads/{tid}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** One of your threads with a page of its messages, oldest first. Anyone else's looks like one that doesn't exist. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    tid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChatThreadDetail"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        /** Deletes one of your threads and everything in it, and stops any run in it. Today's spending stays counted. */
        delete: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    tid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                204: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        options?: never;
        head?: never;
        /** Renames one of your threads. */
        patch: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    tid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ChatRenameBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChatThreadDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        trace?: never;
    };
    "/api/v1/vaults/{id}/contradictions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ContradictionPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    type: string;
                    /** @description `<field>:<op>:<value>`, where op is `eq`, `lt`, `lte`, `gt` or `gte`. Repeat it to require several. The field must be filterable. */
                    filter?: string;
                    cursor?: string;
                    limit?: number;
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EntityPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post: {
            parameters: {
                query?: {
                    /** @description Validate and decide, but write nothing. */
                    dry_run?: boolean;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NewEntityBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EntityProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    mention: string;
                    /** @description Only entities of this type. */
                    type?: string;
                    /** @description 1 to 50. The default is 10. */
                    limit?: number;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ResolveResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/same-as": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    type: string;
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description By field and value, the only order. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SameAsPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/{eid}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** An old id gets the entity it now leads to, with `redirected_from` set. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EntityDetailDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/{eid}/edit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: {
                    /** @description Validate and decide, but write nothing. */
                    dry_run?: boolean;
                };
                header?: never;
                path: {
                    id: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["EntityEditBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/{eid}/merge": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Proposes merging this entity into another. It always waits for a person's approval. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["MergeBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/{eid}/redirect": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["RedirectDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entities/{eid}/split": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Proposes moving claims and aliases to a new entity. It always waits for a person's approval. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    eid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["SplitBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EntityProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entity-types": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EntityTypePage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        /** A new type or the next version of one. An additive change by a person commits, and a breaking one waits for review. The response says which. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NewEntityTypeBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entity-types/{name}/preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Classifies a change as additive or breaking and says what would happen to it, writing nothing. A submission of the same schema gets the same answer. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    name: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["TypePreviewBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["TypePreviewDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/entity-types/{name}/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    name: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EntityTypeVersions"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/flagged-pages": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["FlaggedPagePage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/jobs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                    kind?: string;
                    /** @description `queued`, `running`, `done`, `dead` or `cancelled`. `dead` is the dead-letter list. */
                    state?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["JobPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/jobs/{jid}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    jid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["JobDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/jobs/{jid}/cancel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    jid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["JobDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/jobs/{jid}/retry": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    jid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["JobDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/objects/{oid}/neighborhood": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Every link at one object, both ways, one hop. Kinds default to wiki links, claim references and contradictions. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                    /** @description Comma-separated edge kinds. The default is `wiki_link,claim_ref,contradicts`. */
                    kinds?: string;
                };
                header?: never;
                path: {
                    id: string;
                    oid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["NeighborPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageSummaryPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        /** A person's page commits at once. An agent's waits for review until it has earned auto-commit. The response says which. */
        post: {
            parameters: {
                query?: {
                    /** @description Validate and decide, but write nothing. */
                    dry_run?: boolean;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["NewPageBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageProposalResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/by-slug/{slug}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Where a slug leads, whether current or old. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    slug: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageRedirectDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/import": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Reads a page's Markdown file. Links reach their pages by slug, so files can arrive in any order. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["PageImportBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageImportResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/unresolved-links": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Wiki links whose slug no page holds, and claim references whose claim doesn't exist. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["UnresolvedLinkPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The page now, or at a past revision. Claim references show what stands today either way. */
        get: {
            parameters: {
                query?: {
                    /** @description A past revision. The current one by default. */
                    revision?: number;
                };
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageDetailDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/backlinks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** What links here, through any slug the page has ever had. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["BacklinkPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/delete": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Proposes deleting the page. It always waits for review. Links to it read as broken until a page takes the slug. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["PageDeleteBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/diff": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** What changed between two revisions, block by block. */
        get: {
            parameters: {
                query: {
                    from: number;
                    to: number;
                };
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageDiffDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/edit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** A stale `base_version` answers with a conflict that carries the page's current state. */
        post: {
            parameters: {
                query?: {
                    /** @description Validate and decide, but write nothing. */
                    dry_run?: boolean;
                };
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["PageEditBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/export": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The page as a Markdown file with front matter, which import reads back. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "text/markdown": string;
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/links": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** What this page links to and cites, each saying whether it leads anywhere. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["OutgoingLinkPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/rename": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** A new title and slug. The old slug keeps leading here, so no link breaks. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["PageRenameBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/revert": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Makes an old revision current again, as a new revision. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["PageRevertBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/pages/{pid}/revisions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Oldest first. A deleted page's history is still here. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["RevisionPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/proposals": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: {
                    /** @description Validate and decide, but write nothing. */
                    dry_run?: boolean;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ProposalBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ProposalResult"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/reanchor-failures": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ReanchorFailurePage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/renditions/{rid}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    rid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["RenditionDetail"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/renditions/{rid}/derivations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    rid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["DerivationPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/renditions/{rid}/segments": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    rid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SegmentPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/auto-commit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Each combination of extractor, prompt version and type: its precision, Wilson bound, whether it's on, and every time that changed, with the threshold it's judged by. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AutoCommitPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/auto-commit/{combination}/disable": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Stops auto-commit for this combination at once. Its counts start again, and there's no route to turn it on: only new samples clearing the threshold can. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    combination: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["DisableBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["AutoCommitDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/batch": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Up to 200 decisions, each attributed and each on its own, so one conflict leaves the others as they are. A malformed request refuses the lot. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["BatchBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["BatchResultsDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/counts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["QueueCountsDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/items/{cs}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** One item: the proposal, the reviewer agent's verdict, what its quotes rest on, and where its combination stands. */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ReviewDetailDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/items/{cs}/edit-approve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Approves a person's corrected version. What was proposed stays on record, and it counts against the extractor. Only a person can do this. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    cs: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["EditApproveBody"];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ChangeSetDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/review/queue": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The queue with `state=pending`, or any other ChangeSets the filters ask for, in creation order. Each comes with the reviewer agent's verdict if it has one. */
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                    /** @description `pending`, `committed` or `rejected`. */
                    state?: string;
                    extractor?: string;
                    prompt_version?: string;
                    /** @description `claim`, `entity:<type>`, `page`, `link` or `source`. */
                    item_type?: string;
                    source_id?: string;
                    proposer?: string;
                    /** @description `approve`, `reject`, `abstain`, or `none` for items with no verdict yet. */
                    verdict?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ReviewItemPage"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/search": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    q: string;
                    /** @description `hybrid`, the default, or `lexical` or `vector` for comparison and debugging. */
                    mode?: string;
                    /** @description Ranked items per page, 1 to 100. The default is 20. */
                    limit?: number;
                    /** @description The most estimated tokens per page. The default is 4000. */
                    budget_tokens?: number;
                    cursor?: string;
                    /** @description `segment` or `claim`. */
                    kind?: string;
                    /** @description A segment's media type, or a claim's predicate. */
                    type?: string;
                    source_id?: string;
                    trust_tier?: string;
                    epistemic?: string;
                    /** @description Keeps what's valid at this time, in milliseconds since 1970. */
                    valid_at_ms?: number;
                    /** @description Answer as the vault stood just after this event. Slow: it reads the log. */
                    as_of?: number;
                    /** @description As `as_of`, by time in milliseconds since 1970. */
                    as_of_ms?: number;
                    /** @description Wait until the indexes have seen this event, for reading your own writes. */
                    min_seq?: number;
                    /** @description How long `min_seq` may wait, 0 to 30000. The default is 2000. */
                    min_seq_wait_ms?: number;
                    /** @description Also rerank the top 20 with a cross-encoder. Slower. */
                    rerank?: boolean;
                    /** @description Add per-signal scores, the planner's path and timings. */
                    debug?: boolean;
                    /** @description Add counts for the typed filters, over the candidates found. */
                    facets?: boolean;
                    /** @description Whether to follow Edges from the best candidates: `auto`, the default, leaves it to the planner's rules, or `on` or `off`. */
                    expand?: string;
                    /** @description `<type>.<field>`: search within one entity field. Keyword only, since the vector index has no fields. */
                    attr?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["EvidencePackageDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                429: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                503: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/search/lexical": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    q: string;
                    /** @description 1 to 1000. The default is 20. */
                    limit?: number;
                    /** @description `segment` or `claim`. All kinds by default. */
                    kind?: string;
                    source_id?: string;
                    epistemic?: string;
                    /** @description Keeps what's valid at this time, in milliseconds since 1970. */
                    valid_at_ms?: number;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["LexicalResultsDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                503: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/search/vector": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query: {
                    q: string;
                    /** @description 1 to 1000. The default is 20. */
                    limit?: number;
                    /** @description `segment` or `claim`. All kinds by default. */
                    kind?: string;
                    source_id?: string;
                    epistemic?: string;
                    /** @description Keeps what's valid at this time, in milliseconds since 1970. */
                    valid_at_ms?: number;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["LexicalResultsDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                503: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/segments/{seg}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    seg: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SegmentDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/segments/{seg}/claims": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    seg: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ClaimRefPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SourcePage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post: {
            parameters: {
                query: {
                    /** @description The file's name, shown to people. */
                    name: string;
                    /** @description `verified`, `trusted`, `unverified` or `untrusted`. The default is `unverified`. */
                    trust_tier?: string;
                    /** @description The Source this is a new version of. */
                    replaces?: string;
                    /** @description A recording's language, such as `sv`, which helps its transcriber. Without it, the transcriber works it out. */
                    language?: string;
                };
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/octet-stream": number[];
                };
            };
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["UploadResult"];
                    };
                };
                201: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["UploadResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                413: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                415: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                429: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SourceDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/claims": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                    /** @description `created`, the default. */
                    sort?: string;
                };
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ClaimRefPage"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/extraction": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ExtractionStatusDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/extraction/rerun": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Queues the document's current rendition again. Segments this prompt already has results for cost nothing and repeat nothing. Asking while a run is open returns that run. */
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["JobDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/file": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description The original file, as an attachment */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                /** @description Part of it, for a Range request */
                206: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/media": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["MediaInfoDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/meeting": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["MeetingDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["MeetingBody"];
                };
            };
            responses: {
                /** @description Proposed. A person's commits at once and an agent's waits for review. */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["MeetingProposalResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/meeting/participants/{pid}/link": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                    pid: string;
                };
                cookie?: never;
            };
            requestBody: {
                content: {
                    "application/json": components["schemas"]["ParticipantLinkBody"];
                };
            };
            responses: {
                /** @description Proposed. A person's commits at once and an agent's waits for review. */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["MeetingProposalResult"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/pages": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["PageListDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/pages/{page}/image": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description One of the allowed values. The default is 150. */
                    dpi?: number;
                };
                header?: never;
                path: {
                    id: string;
                    sid: string;
                    page: number;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description A PNG of the page */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "image/png": unknown;
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/pages/{page}/thumbnail": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                    page: number;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description A small PNG of the page */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "image/png": unknown;
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/renditions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["RenditionList"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/reprocess": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["JobDto"];
                    };
                };
                403: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description The original recording, as it was uploaded, to play */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                /** @description Part of it, for a Range request. A player seeks to a cited time with these. */
                206: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                /** @description The range starts past the end */
                416: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content?: never;
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/transcript": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: {
                    /** @description Text around this time, in milliseconds. Goes with `window_ms`. */
                    around_ms?: number;
                    /** @description How far either side of `around_ms`. The default is 30000. */
                    window_ms?: number;
                    /** @description Text from this time on. */
                    from_ms?: number;
                    /** @description Text up to this time. */
                    to_ms?: number;
                    /** @description Include each word's own times. */
                    words?: boolean;
                    /** @description From `next_cursor` of the previous page. */
                    cursor?: string;
                    /** @description 1 to 200. The default is 50. */
                    limit?: number;
                };
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["TranscriptDto"];
                    };
                };
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
                /** @description Not transcribed yet */
                409: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/sources/{sid}/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                    sid: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SourceVersions"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/vaults/{id}/upload-limits": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    id: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["UploadLimitsDto"];
                    };
                };
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/version": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["VersionDto"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/whoami": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["WhoAmI"];
                    };
                };
                401: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorBody"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /** @description A page of the live board. */
        ActivityDocumentPage: {
            /**
             * Format: int64
             * @description The log position this reflects, which a stream can follow from with `after`. -1 is before the first event.
             */
            head: number;
            items: components["schemas"]["DocumentActivityDto"][];
            next_cursor?: string | null;
        };
        AssertionList: {
            items: Record<string, never>[];
        };
        AuditBudgetDto: {
            /**
             * Format: int64
             * @description Whole days since the epoch, in UTC.
             */
            day: number;
            /** Format: int64 */
            day_remaining: number;
            /** Format: int64 */
            day_spent: number;
            /** Format: int64 */
            day_token_budget: number;
            /** Format: int64 */
            run_token_budget: number;
        };
        /** @description One sampled claim and what was found about it. A finding is one of these whose outcome isn't `passed`. */
        AuditCheckDto: {
            auto_committed: boolean;
            /** @description The review item a person decides this on. */
            changeset_id?: string | null;
            checks: components["schemas"]["AuditCheckResultDto"][];
            claim_id: string;
            combination?: components["schemas"]["AuditCombinationDto"] | null;
            counted_correct: boolean;
            /** Format: int64 */
            finished_at_ms?: number | null;
            id: string;
            /** @description `sampled`, `passed`, `suspect` or `failed`. */
            outcome: string;
            /** Format: int32 */
            rank: number;
            run_id: string;
            /** Format: int64 */
            weight_micro: number;
        };
        AuditCheckPage: {
            items: components["schemas"]["AuditCheckDto"][];
            next_cursor?: string | null;
        };
        AuditCheckResultDto: {
            cause?: string | null;
            check: string;
            /** @description May hold a document's or a model's words, so show it as plain text. */
            detail: string;
            model?: string | null;
            prompt_hash?: string | null;
            /** @description `pass`, `fail` or `inconclusive`. */
            status: string;
            tokens?: components["schemas"]["TokenUsageDto"] | null;
        };
        AuditCombinationDto: {
            extractor: string;
            item_type: string;
            prompt_version: string;
        };
        AuditRunDetailDto: {
            run: components["schemas"]["AuditRunDto"];
            /** @description The claims it sampled, in rank order. The checks route has what was found about each. */
            sample: string[];
        };
        AuditRunDto: {
            /** Format: int64 */
            created_at_ms: number;
            /** Format: int32 */
            failed: number;
            /** Format: int64 */
            finished_at_ms?: number | null;
            id: string;
            /** Format: int32 */
            llm_calls: number;
            /** Format: int32 */
            llm_skipped: number;
            /** Format: int32 */
            missed_slots: number;
            model?: string | null;
            /** Format: int32 */
            passed: number;
            /** Format: int64 */
            population: number;
            requested_by: string;
            /** @description Combinations this run took auto-commit from. */
            revoked: string[];
            /** Format: int64 */
            run_token_budget: number;
            /** Format: int32 */
            sample_size: number;
            /** Format: int32 */
            sampled: number;
            /** @description A decimal number, as text. */
            seed: string;
            /**
             * Format: int64
             * @description Claims of revoked combinations still to go back to review. Later runs carry on with them.
             */
            siblings_remaining: number;
            /** Format: int32 */
            siblings_requeued: number;
            /** Format: int64 */
            started_at_ms?: number | null;
            /** @description `queued`, `running`, `finished`, or `failed` if its job gave up. */
            state: string;
            /** Format: int64 */
            state_seq?: number | null;
            /** @description Why its LLM checks ended early: `run_budget`, `day_budget`, `llm_unavailable` or `llm_refused`. Running out of budget isn't a failure. */
            stop?: string | null;
            stop_detail?: string | null;
            /** Format: int32 */
            suspect: number;
            tokens: components["schemas"]["TokenUsageDto"];
            /** @description `scheduled` or `on_demand`. */
            trigger: string;
            vault_findings: components["schemas"]["VaultFindingDto"][];
        };
        AuditRunFact: {
            /** Format: int32 */
            failed: number;
            /** Format: int64 */
            finished_at_ms?: number | null;
            id: string;
            job_id?: string | null;
            /** Format: int32 */
            llm_calls: number;
            /** Format: int32 */
            passed: number;
            /** Format: int32 */
            sample_size: number;
            /** Format: int32 */
            sampled: number;
            /** @description `queued`, `running` or `finished`. */
            state: string;
            /** @description Why its LLM checks stopped short, in a word. */
            stop?: string | null;
            /** Format: int32 */
            suspect: number;
        };
        AuditRunPage: {
            items: components["schemas"]["AuditRunDto"][];
            next_cursor?: string | null;
        };
        /** @description How often to audit, how much, and what it may spend. */
        AuditScheduleBody: {
            /** Format: int64 */
            day_token_budget: number;
            enabled: boolean;
            /** Format: int64 */
            interval_ms: number;
            /** Format: int64 */
            run_token_budget: number;
            /** Format: int32 */
            sample_size: number;
            /** @description Days until claims of each predicate go stale. */
            ttl_days?: {
                [key: string]: number;
            };
        };
        AuditScheduleDto: {
            /** Format: int64 */
            day_token_budget: number;
            enabled: boolean;
            /** Format: int64 */
            interval_ms: number;
            /** Format: int64 */
            next_run_at_ms?: number | null;
            /** Format: int64 */
            run_token_budget: number;
            /** Format: int32 */
            sample_size: number;
            /**
             * Format: int64
             * @description Slots passed over since it began, because the server was away or a run was still open.
             */
            total_missed_slots: number;
            ttl_days: {
                [key: string]: number;
            };
        };
        AutoCommitChangeDto: {
            /** Format: int64 */
            at_ms: number;
            by: string;
            /** Format: int64 */
            correct: number;
            /** Format: int64 */
            incorrect: number;
            /** @description `earned`, `fell_below` or `disabled`. */
            kind: string;
            reason: string;
        };
        AutoCommitDto: {
            /**
             * Format: int64
             * @description Since it was last disabled. These decide whether it's on.
             */
            correct: number;
            extractor: string;
            /** @description When it was earned, fell below the threshold or was disabled, oldest first. */
            history: components["schemas"]["AutoCommitChangeDto"][];
            id: string;
            /** Format: int64 */
            incorrect: number;
            item_type: string;
            on: boolean;
            /**
             * Format: double
             * @description Absent with no samples.
             */
            precision?: number | null;
            prompt_version: string;
            /** Format: int64 */
            total_correct: number;
            /** Format: int64 */
            total_incorrect: number;
            /** Format: double */
            wilson_bound: number;
        };
        AutoCommitPage: {
            items: components["schemas"]["AutoCommitDto"][];
            next_cursor?: string | null;
            /**
             * Format: double
             * @description What `wilson_bound` has to reach.
             */
            threshold: number;
        };
        BacklinkDto: {
            /** @description The block holding the link, which is its evidence. */
            block_id?: string | null;
            edge_id: string;
            from_page: string;
            from_slug?: string | null;
            from_title?: string | null;
            /** @description The slug as the link wrote it, which may be an old one. */
            via_slug: string;
        };
        BacklinkPage: {
            items: components["schemas"]["BacklinkDto"][];
            next_cursor?: string | null;
        };
        BatchBody: {
            decisions: components["schemas"]["BatchDecisionBody"][];
        };
        /** @description One decision in a batch. */
        BatchDecisionBody: {
            /** @description `approve`, `reject` or `edit_approve`. */
            decision: string;
            id: string;
            /** @description With `edit_approve`. */
            ops?: Record<string, never>[] | null;
            reason?: string | null;
        };
        /** @description How one decision went. One failing leaves the others as they are. */
        BatchResultDto: {
            changeset?: components["schemas"]["ChangeSetDto"] | null;
            /** @description Set when it failed, with the same codes as the single routes. */
            error_code?: string | null;
            error_message?: string | null;
            id: string;
        };
        BatchResultsDto: {
            results: components["schemas"]["BatchResultDto"][];
        };
        BlockChangeDto: {
            after?: string | null;
            before?: string | null;
            block_id: string;
            /** @description `added`, `removed`, `changed` or `moved`. */
            change: string;
        };
        BlockDto: {
            id: string;
            /** @description `heading`, `paragraph`, `list`, `quote`, `code`, `table` or `rule`. */
            kind: string;
            /** @description Verbatim Markdown. */
            text: string;
        };
        BudgetDto: {
            limit: number;
            smallest_item_needs?: number | null;
            truncated: boolean;
            used: number;
        };
        CacheStatsDto: {
            tiers: components["schemas"]["CacheTierDto"][];
        };
        /** @description One cache tier's counters since the server started. */
        CacheTierDto: {
            /**
             * Format: int64
             * @description The tier's limit. 0 means it's off.
             */
            cap_bytes: number;
            /**
             * Format: int64
             * @description Disk files that couldn't be trusted, each also counted as a miss.
             */
            corrupt: number;
            /** Format: int64 */
            entries: number;
            /** Format: int64 */
            evictions: number;
            /** Format: int64 */
            hits: number;
            /** Format: int64 */
            misses: number;
            /**
             * Format: int64
             * @description Entries too big for the tier to keep.
             */
            rejected: number;
            /** Format: int64 */
            size_bytes: number;
            tier: string;
        };
        CandidateDto: {
            entity: components["schemas"]["EntityDto"];
            exact: boolean;
            /** @description The identity field or `$alias` the mention matches exactly. Absent on a keyword match. */
            matched_on?: string | null;
            /**
             * Format: float
             * @description The keyword score, comparable only within this answer.
             */
            score: number;
        };
        ChangeSetDto: {
            /** Format: int64 */
            commit_seq?: number | null;
            /** Format: int64 */
            decided_at_ms?: number | null;
            decided_by?: string | null;
            decision_reason?: string | null;
            /** @description The model behind an extraction, or otherwise whoever proposed it. Absent from before spec 025. */
            extractor?: string | null;
            id: string;
            idempotency_key: string;
            item_types?: string[] | null;
            ops: Record<string, never>[];
            /** @description What was proposed, when a person changed it before approving. `ops` then holds what was approved. */
            original_ops?: Record<string, never>[] | null;
            prompt_version?: string | null;
            /** Format: int64 */
            proposed_at_ms: number;
            proposer: string;
            proposer_is_machine: boolean;
            rationale: string;
            /** @description The person whose chat asked for this, when the proposer is the chat agent acting for them. */
            requested_by?: string | null;
            reverted_by?: string | null;
            reverts?: string | null;
            /** @description `auto_commit`, `review` or `reject`. */
            risk: string;
            source_ids?: string[] | null;
            /** @description `pending`, `committed` or `rejected`. */
            state: string;
            validators: Record<string, never>[];
        };
        ChangeSetFact: {
            /** Format: int64 */
            commit_seq?: number | null;
            /** Format: int64 */
            decided_at_ms?: number | null;
            id: string;
            machine: boolean;
            /** Format: int32 */
            ops: number;
            /** Format: int64 */
            proposed_at_ms: number;
            /** @description `pending`, `committed` or `rejected`. */
            state: string;
        };
        ChangeSetPage: {
            items: components["schemas"]["ChangeSetDto"][];
            next_cursor?: string | null;
        };
        ChatMessageDto: {
            /** Format: int64 */
            at_ms: number;
            id: string;
            /** @description `user`, `assistant` or `tool`. */
            role: string;
            run_id: string;
            /**
             * Format: int64
             * @description Its place in the thread, which is also the cursor.
             */
            seq: number;
            /** @description Why this reply ended early, such as `cancelled`, `max_tokens` or `failed`. */
            stop?: string | null;
            /** @description What was said, or a tool's answer. */
            text: string;
            tool_call?: components["schemas"]["ChatToolCallDto"] | null;
        };
        ChatRenameBody: {
            title: string;
        };
        ChatSettingsBody: {
            /**
             * Format: int64
             * @description Tokens per UTC day for this vault, above zero. Absent takes the server's default.
             */
            day_token_budget?: number | null;
            /** @description Turning this on sends the vault's content to the configured LLM. */
            enabled: boolean;
            /**
             * Format: int64
             * @description Tokens per thread, above zero. Absent takes the server's default.
             */
            thread_token_budget?: number | null;
        };
        ChatSettingsDto: {
            /** Format: int64 */
            day_token_budget?: number | null;
            /**
             * Format: int64
             * @description The limit today's spending is held to.
             */
            day_tokens_limit: number;
            /**
             * Format: int64
             * @description What this vault's chat has been charged today, which includes what's reserved for calls still going.
             */
            day_tokens_used: number;
            enabled: boolean;
            /** Format: int64 */
            thread_token_budget?: number | null;
        };
        ChatThreadDetail: {
            messages: components["schemas"]["ChatMessageDto"][];
            next_cursor?: string | null;
            thread: components["schemas"]["ChatThreadDto"];
        };
        ChatThreadDto: {
            /** Format: int64 */
            created_at_ms: number;
            id: string;
            title: string;
            /** Format: int64 */
            tokens: number;
            /** Format: int64 */
            updated_at_ms: number;
        };
        ChatThreadPage: {
            items: components["schemas"]["ChatThreadDto"][];
            next_cursor?: string | null;
        };
        ChatToolCallDto: {
            /** @description As the model wrote them, in JSON. */
            args: string;
            id: string;
            name: string;
        };
        /** @description Where an item's text comes from. `anchor` is always there: `located`, `broken` or `none`. */
        CitationDto: {
            anchor: string;
            /** Format: int64 */
            end?: number | null;
            evidence_id?: string | null;
            /** Format: int32 */
            page_from?: number | null;
            /** Format: int32 */
            page_to?: number | null;
            quote?: string | null;
            rects: components["schemas"]["RectDto"][];
            rendition_id?: string | null;
            segment_id?: string | null;
            source_id?: string | null;
            source_name?: string | null;
            source_sha256?: string | null;
            /** @description Who was speaking, when that's known and one person spoke all of it. */
            speaker?: string | null;
            /** Format: int64 */
            start?: number | null;
            /** Format: int64 */
            t_end_ms?: number | null;
            /**
             * Format: int64
             * @description Where it plays in the original recording, in milliseconds. Only a recording has these.
             */
            t_start_ms?: number | null;
            trust_tier?: string | null;
        };
        ClaimRefDto: {
            assertion_id: string;
            evidence_id: string;
            predicate: string;
            subject: string;
            value: Record<string, never>;
        };
        ClaimRefPage: {
            items: components["schemas"]["ClaimRefDto"][];
            next_cursor?: string | null;
        };
        ConflictDto: {
            claim_id: string;
            epistemic?: string | null;
            source_ids: string[];
            text: string;
        };
        ContradictionDto: {
            claim: string;
            other: string;
        };
        ContradictionPage: {
            items: components["schemas"]["ContradictionDto"][];
            next_cursor?: string | null;
        };
        /** @description Shown once, at creation. */
        CreatedTokenDto: {
            details: components["schemas"]["TokenDto"];
            token: string;
        };
        DerivationDto: {
            /** Format: int32 */
            attempts: number;
            /** Format: int64 */
            created_at_ms: number;
            detail?: string | null;
            id: string;
            /** @description The results as validated, each with its segment and exact quote. */
            items: Record<string, never>[];
            job_id: string;
            kind: string;
            model: string;
            /** @description `done`, or `failed` when the tokens were spent with nothing usable back. */
            outcome: string;
            prompt_hash: string;
            rejected: components["schemas"]["RejectedItemDto"][];
            rendition_id: string;
            segment_ids: string[];
            source_id: string;
            tokens: components["schemas"]["TokenUsageDto"];
        };
        DerivationFact: {
            id: string;
            /** Format: int32 */
            items: number;
            job_id?: string | null;
            /** @description `done` or `failed`. */
            outcome: string;
            /** Format: int32 */
            refused: number;
            rendition_id?: string | null;
            /** Format: int32 */
            segments: number;
            source_id?: string | null;
            /** Format: int64 */
            tokens: number;
        };
        DerivationPage: {
            items: components["schemas"]["DerivationDto"][];
            next_cursor?: string | null;
        };
        DisableBody: {
            reason: string;
        };
        /** @description One document on the live board. */
        DocumentActivityDto: {
            /** Format: int32 */
            attempts: number;
            /** @description The job this depends on now, if one does. */
            job_id?: string | null;
            logical_id: string;
            /** Format: int64 */
            next_run_at_ms?: number | null;
            /** Format: int32 */
            page_count?: number | null;
            /** @description Some part of it didn't work, whatever stage it's in now. */
            partial: boolean;
            /** Format: int64 */
            pending_review: number;
            progress?: components["schemas"]["DocumentProgressDto"] | null;
            /** Format: int64 */
            recorded_at_ms: number;
            rendition_id?: string | null;
            /** Format: int32 */
            segment_count?: number | null;
            /** Format: int32 */
            segments_extracted?: number | null;
            source_id: string;
            /** @description `stored`, `parsing`, `ocr`, `transcription`, `segmenting`, `indexing`, `embedding`, `extraction`, `review` or `done`. */
            stage: string;
            /** @description `queued`, `active`, `waiting`, `retrying`, `failed`, `partial` or `done`. */
            status: string;
            /** Format: int32 */
            version_no: number;
            /** Format: int64 */
            waiting_since_ms?: number | null;
        };
        /** @description How far a worker has got. */
        DocumentProgressDto: {
            /** Format: int64 */
            done: number;
            stage: string;
            /** Format: int64 */
            time_ms: number;
            /** Format: int64 */
            total?: number | null;
        };
        /** @description One event from the log, reduced to what can safely be said about it. Its `seq` is the frame's SSE id. */
        DurableFrame: {
            audit_runs?: components["schemas"]["AuditRunFact"][];
            changesets?: components["schemas"]["ChangeSetFact"][];
            /** @description How many rows of each kind the event wrote. */
            counts?: {
                [key: string]: number;
            };
            derivations?: components["schemas"]["DerivationFact"][];
            extractions?: components["schemas"]["ExtractionFact"][];
            jobs?: components["schemas"]["JobFact"][];
            /** @description The event's kind, or `other` for one this server doesn't list. */
            kind: string;
            /**
             * Format: int32
             * @description How many items didn't fit in the lists above.
             */
            omitted?: number;
            renditions?: components["schemas"]["RenditionFact"][];
            /** Format: int64 */
            seq: number;
            sources?: components["schemas"]["SourceFact"][];
            /** Format: int64 */
            time_ms: number;
        };
        EditApproveBody: {
            /** @description What to approve instead of what was proposed, which stays on record. */
            ops: Record<string, never>[];
            reason?: string | null;
        };
        /** @description The last frame of a stream, and why. */
        EndFrame: {
            /** @description `auth_ended` (the token or session is over), `scope_lost` (it can't read this vault now), `vault_gone`, `unverifiable` (the store was away too long to check), `unavailable` or `server_stopping`. */
            reason: string;
        };
        EntityDetailDto: {
            aliases: string[];
            changeset_id: string;
            fields: components["schemas"]["EntityFieldDto"][];
            id: string;
            merged_from: string[];
            /** @description The id asked for, when it has been merged into this one. */
            redirected_from?: string | null;
            type: string;
            /** Format: int32 */
            type_version: number;
            /** Format: int64 */
            updated_at_ms: number;
            /** Format: int32 */
            version: number;
        };
        /** @description An entity's current state, as lists show it. */
        EntityDto: {
            aliases: string[];
            /** @description Each field's current value. */
            attributes: Record<string, never>;
            /** @description The fields with a claim that disagrees with the current value. */
            conflicted_fields: string[];
            id: string;
            merged_from: string[];
            type: string;
            /** Format: int32 */
            type_version: number;
            /** Format: int64 */
            updated_at_ms: number;
            /** Format: int32 */
            version: number;
        };
        EntityEditBody: {
            /** @description Replaces the aliases. Leave it out to keep them. */
            aliases?: string[] | null;
            /** Format: int32 */
            base_version: number;
            fields?: components["schemas"]["FieldInputDto"][];
            idempotency_key: string;
            rationale: string;
        };
        /** @description A field: its current value, why it's the one, and every other claim. `conflicts` lists those that disagree, and `agreeing` those that don't. */
        EntityFieldDto: {
            agreeing: components["schemas"]["FieldClaimDto"][];
            conflicts: components["schemas"]["FieldClaimDto"][];
            current: components["schemas"]["FieldClaimDto"];
            field: string;
            value: Record<string, never>;
            why: string;
        };
        EntityPage: {
            items: components["schemas"]["EntityDto"][];
            next_cursor?: string | null;
        };
        /** @description A proposal about an entity, and the entity it's about. */
        EntityProposalResult: components["schemas"]["ProposalResult"] & {
            entity_id: string;
        };
        EntityTypeDto: {
            /** @description `seed`, `new`, `additive` or `breaking`. */
            change: string;
            /** @description The ChangeSet that made it, absent on a seed type's built-in first version. */
            changeset_id?: string | null;
            /** Format: int64 */
            created_at_ms: number;
            name: string;
            reasons: string[];
            /** @description The version's JSON Schema, with the annotations `searchable`, `filterable` and `identity` on its fields. */
            schema: Record<string, never>;
            /** Format: int32 */
            version: number;
        };
        EntityTypePage: {
            items: components["schemas"]["EntityTypeDto"][];
            next_cursor?: string | null;
        };
        EntityTypeVersions: {
            /** @description Oldest first. */
            items: components["schemas"]["EntityTypeDto"][];
        };
        /** @description The one error format. */
        ErrorBody: {
            error: components["schemas"]["ErrorDetail"];
        };
        ErrorDetail: {
            /** @description A stable machine-readable code, such as `not_found`. */
            code: string;
            /** @description On `stale_version`: the row as it is now, so the caller can rebase. */
            current?: Record<string, never> | null;
            message: string;
            request_id: string;
        };
        EventDto: {
            actor: string;
            kind: string;
            payload: Record<string, never>;
            /** Format: int32 */
            schema_version: number;
            /** Format: int64 */
            seq: number;
            /** Format: int64 */
            time_ms: number;
        };
        EventPage: {
            items: components["schemas"]["EventDto"][];
            next_cursor?: string | null;
        };
        EvidenceInputDto: {
            /** @description Verbatim from the segment, apart from runs of whitespace. */
            quote: string;
            segment_id: string;
            /** @description `supports`, the default, or `refutes`. */
            stance?: string | null;
        };
        EvidenceItemDto: {
            citations: components["schemas"]["CitationDto"][];
            conflicts: components["schemas"]["ConflictDto"][];
            epistemic?: string | null;
            estimated_tokens: number;
            heading?: string | null;
            id: string;
            /** @description `claim`, `segment`, `entity`, `section`, `page` or `block`. */
            kind: string;
            language?: string | null;
            /** @description The wiki page a page or block item belongs to. */
            page_id?: string | null;
            /** Format: float */
            score?: number | null;
            text: string;
            trust_tiers: string[];
            /** Format: int64 */
            valid_from_ms?: number | null;
            /** Format: int64 */
            valid_to_ms?: number | null;
        };
        EvidenceLineageDto: {
            /** @description The quote no longer resolves. Reported rather than guessed. */
            broken: boolean;
            evidence_id: string;
            /** @description The source's media type. A recording's bytes play from `/sources/{sid}/stream`. */
            media_type: string;
            parser: string;
            quote: string;
            /** @description Displayed coordinates in 1/100 pt, one per line. */
            rects: components["schemas"]["RectDto"][];
            rendition_id: string;
            segment_id: string;
            source_id: string;
            source_name: string;
            source_sha256: string;
            /** @description Who was speaking, when that's known and one person spoke all of it. */
            speaker?: string | null;
            stance: string;
            /** @description A newer rendition exists. This still resolves. */
            superseded: boolean;
            /** Format: int64 */
            t_end_ms?: number | null;
            /**
             * Format: int64
             * @description Where the quote plays in the original recording, in milliseconds. Only a recording has these.
             */
            t_start_ms?: number | null;
            trust_tier: string;
        };
        /** @description The answer to a search: claims first, then segments, then context sections, within a token budget. */
        EvidencePackageDto: {
            /** Format: int64 */
            as_of?: number | null;
            budget: components["schemas"]["BudgetDto"];
            debug?: components["schemas"]["SearchDebugDto"] | null;
            /** @description Signals that couldn't answer, so the result rests on the others. */
            degraded: string[];
            facets?: components["schemas"]["FacetsDto"] | null;
            items: components["schemas"]["EvidenceItemDto"][];
            /** @description Nothing here looks like an answer. */
            low_relevance: boolean;
            next_cursor?: string | null;
            notices: string[];
            /** @description `exact`, `identifier`, `natural`, `filtered` or `as_of`. */
            path: string;
            /** Format: int64 */
            watermark?: number | null;
        };
        ExtractionFact: {
            /** Format: int32 */
            batches_failed: number;
            job_id?: string | null;
            /** Format: int32 */
            proposed: number;
            /** Format: int32 */
            rejected: number;
            rendition_id: string;
            /** Format: int32 */
            segments_done: number;
            /** Format: int32 */
            segments_total: number;
            source_id?: string | null;
            /** @description `budget`, `failed_batches` or `superseded`, when a run ended short. */
            stop?: string | null;
            /** Format: int64 */
            token_budget: number;
            /** Format: int64 */
            tokens: number;
        };
        ExtractionStatusDto: {
            /** Format: int32 */
            batches_failed: number;
            job_id?: string | null;
            last_error?: string | null;
            model?: string | null;
            prompt_hash?: string | null;
            /**
             * Format: int32
             * @description New proposals this run made, which wait in the review queue.
             */
            proposed: number;
            /**
             * Format: int32
             * @description Results refused, such as a quote that isn't in its segment.
             */
            rejected: number;
            /** @description The current rendition, once there is one. */
            rendition_id?: string | null;
            /** Format: int32 */
            segments_done: number;
            /** Format: int32 */
            segments_total: number;
            source_id: string;
            /** @description `not_started`, `queued`, `running`, `waiting` for the LLM, `partial`, `done`, `failed` or `cancelled`. */
            state: string;
            /** @description Why a `partial` run stopped: `budget`, `failed_batches` or `superseded`. */
            stop_reason?: string | null;
            /** Format: int64 */
            token_budget: number;
            /** @description What this run has spent. */
            tokens: components["schemas"]["TokenUsageDto"];
            /** Format: int64 */
            updated_at_ms?: number | null;
            /** Format: int64 */
            waiting_since_ms?: number | null;
        };
        FacetCountDto: {
            /** Format: int64 */
            count: number;
            value: string;
        };
        /** @description Counts over the candidates found, not the whole vault. */
        FacetsDto: {
            epistemic: components["schemas"]["FacetCountDto"][];
            kind: components["schemas"]["FacetCountDto"][];
            source_id: components["schemas"]["FacetCountDto"][];
            trust_tier: components["schemas"]["FacetCountDto"][];
            type: components["schemas"]["FacetCountDto"][];
        };
        /** @description A claim on a field. Every value traces to one, and a person's claim may have no evidence. */
        FieldClaimDto: {
            assertion_id: string;
            changeset_id?: string | null;
            creator?: string | null;
            /** @description `verified`, `asserted`, `inferred` or `disputed`. */
            epistemic?: string | null;
            evidence: components["schemas"]["FieldEvidenceDto"][];
            /** @description A machine made it. */
            machine?: boolean | null;
            value: Record<string, never>;
        };
        /** @description One piece of evidence, and the way to its source. `assertions/{assertion_id}/lineage` and `.../evidence/{evidence_id}/highlight` give the full region. */
        FieldEvidenceDto: {
            /** @description Re-anchoring on a new rendition failed. */
            anchor_broken: boolean;
            evidence_id: string;
            /** Format: int32 */
            page_from?: number | null;
            /** Format: int32 */
            page_to?: number | null;
            quote: string;
            rendition_id?: string | null;
            segment_id: string;
            source_id?: string | null;
            source_name?: string | null;
            /** @description `supports` or `refutes`. */
            stance: string;
            trust_tier?: string | null;
        };
        /** @description One proposed value. It's added beside any others on the field, unless it says which claim it replaces. */
        FieldInputDto: {
            evidence?: components["schemas"]["EvidenceInputDto"][];
            field: string;
            replaces?: components["schemas"]["ReplacesDto"] | null;
            value: Record<string, never>;
        };
        FlaggedPageDto: {
            outcome: Record<string, never>;
            /** Format: int32 */
            page: number;
            rendition_id: string;
            source_id: string;
        };
        FlaggedPagePage: {
            items: components["schemas"]["FlaggedPageDto"][];
            next_cursor?: string | null;
        };
        GrantDto: {
            scope: components["schemas"]["ScopeDto"];
            vault_id: string;
        };
        GroupRecallDto: {
            /** Format: int64 */
            audited: number;
            /** Format: int64 */
            caught: number;
            /** Format: int64 */
            planted: number;
            /** Format: double */
            recall?: number | null;
        };
        HealthDto: {
            status: string;
        };
        /** @description Says the stream is alive when nothing else has. */
        HeartbeatFrame: {
            /** Format: int64 */
            head: number;
        };
        /** @description The first frame of every stream. */
        HelloFrame: {
            /**
             * Format: int64
             * @description Where the log's reader was as this stream opened. -1 is before the first event.
             */
            head: number;
            /** @description Whether this stream carries on from a position the client named. */
            resumed: boolean;
        };
        HighlightDto: {
            broken: boolean;
            /** Format: int32 */
            page: number;
            /** Format: int32 */
            page_height: number;
            /** Format: int32 */
            page_width: number;
            rects: components["schemas"]["RectDto"][];
            source_id: string;
            speaker?: string | null;
            /** Format: int64 */
            t_end_ms?: number | null;
            /**
             * Format: int64
             * @description A recording's highlight is a time range and has no page or rects.
             */
            t_start_ms?: number | null;
        };
        InversePreview: {
            blocked_because?: string | null;
            mutations: Record<string, never>[];
            reversible: boolean;
        };
        JobDto: {
            attempts: Record<string, never>[];
            /** Format: int64 */
            created_at_ms: number;
            /** Format: int64 */
            finished_at_ms?: number | null;
            id: string;
            input: Record<string, never>;
            kind: string;
            /** Format: int32 */
            max_attempts: number;
            /** Format: int64 */
            next_run_at_ms: number;
            /** @description `queued`, `running`, `done`, `dead` or `cancelled`. */
            state: string;
            /**
             * Format: int64
             * @description Set while the job waits for something that's away, such as the LLM.
             */
            waiting_since_ms?: number | null;
        };
        /** @description A job at one moment. Its error messages, its input and who holds it stay out, as an error can quote a document. */
        JobFact: {
            /** Format: int32 */
            attempts: number;
            audit_run_id?: string | null;
            changeset_id?: string | null;
            /**
             * Format: int32
             * @description How many objects an embedding job holds.
             */
            docs?: number | null;
            /** Format: int64 */
            finished_at_ms?: number | null;
            id: string;
            /** @description `rendition`, `transcription`, `embed`, `extract`, `review`, `audit` or `other`. */
            kind: string;
            /** Format: int32 */
            max_attempts: number;
            /** Format: int64 */
            next_run_at_ms: number;
            rendition_id?: string | null;
            source_id?: string | null;
            /** @description The stage this job's work belongs to, when it's about a document. */
            stage?: string | null;
            /** @description `queued`, `running`, `done`, `dead` or `cancelled`. */
            state: string;
            /** @description `queued`, `active`, `waiting`, `retrying`, `failed` or `done`. */
            status: string;
            /** Format: int64 */
            waiting_since_ms?: number | null;
        };
        JobPage: {
            items: components["schemas"]["JobDto"][];
            next_cursor?: string | null;
        };
        KindRecallDto: {
            /** Format: int64 */
            audited: number;
            /** Format: int64 */
            caught: number;
            kind: string;
            /** @description Only an LLM check can catch this kind. */
            needs_llm: boolean;
            /** Format: int64 */
            planted: number;
            /** Format: double */
            recall?: number | null;
        };
        LexicalHitDto: {
            id: string;
            /** @description `segment` or `claim`. */
            kind: string;
            /** Format: float */
            score: number;
        };
        LexicalResultsDto: {
            hits: components["schemas"]["LexicalHitDto"][];
            /**
             * Format: int64
             * @description The last event this answer reflects, absent before the first.
             */
            watermark?: number | null;
        };
        LineageDto: {
            assertion: Record<string, never>;
            changeset_id: string;
            decided_by?: string | null;
            evidence: components["schemas"]["EvidenceLineageDto"][];
            /** @description Who proposed it, and whether a machine did. */
            proposer?: string | null;
            proposer_is_machine?: boolean | null;
            rationale?: string | null;
        };
        /** @description Signs in a browser. No `Debug`, so the token can't reach a log by accident. */
        LoginBody: {
            token: string;
        };
        MediaInfoDto: {
            /** Format: int32 */
            channels?: number | null;
            /**
             * Format: int32
             * @description Stretches read with less accurate times, because the transcriber crashed on them once.
             */
            coarse_spans?: number | null;
            codec?: string | null;
            /**
             * Format: int64
             * @description Empty until transcribed, as decoding is what finds it.
             */
            duration_ms?: number | null;
            /** Format: int64 */
            gap_ms?: number | null;
            /**
             * Format: int32
             * @description Holes in the audio, kept as silence so times stay on the original timeline.
             */
            gaps?: number | null;
            media_type: string;
            /** @description The current rendition, once transcribed. */
            rendition_id?: string | null;
            /** Format: int32 */
            sample_rate?: number | null;
            /** Format: int64 */
            size: number;
            /**
             * Format: int32
             * @description Stretches the transcriber crashed on twice, which have no words.
             */
            skipped_spans?: number | null;
            source_id: string;
        };
        /** @description A full replacement of a meeting's metadata, as a proposal. */
        MeetingBody: {
            /** Format: int32 */
            base_version: number;
            /** Format: int64 */
            date_ms?: number | null;
            idempotency_key: string;
            participants: components["schemas"]["ParticipantInput"][];
            title?: string | null;
        };
        MeetingDto: {
            /** Format: int64 */
            date_ms?: number | null;
            participants: components["schemas"]["ParticipantDto"][];
            source_id: string;
            /** @description With no title set, the file's name. */
            title?: string | null;
            /**
             * Format: int32
             * @description The version to base an edit on.
             */
            version: number;
        };
        MeetingProposalResult: {
            result: components["schemas"]["ProposalResult"];
            source_id: string;
        };
        MergeBody: {
            idempotency_key: string;
            /** @description The entity that stays. */
            into: string;
            rationale: string;
        };
        NameBody: {
            name: string;
        };
        NeighborDto: {
            block_id?: string | null;
            /** @description `out` when the object asked about is where the link starts, or `in`. */
            direction: string;
            edge_id: string;
            from: string;
            kind: string;
            /** @description What's at the other end. Null is a link to a slug no page holds. */
            neighbor?: string | null;
            /** @description The other end's title, when it's a page. */
            title?: string | null;
            to: string;
        };
        NeighborPage: {
            items: components["schemas"]["NeighborDto"][];
            next_cursor?: string | null;
        };
        NewEntityBody: {
            aliases?: string[];
            fields: components["schemas"]["FieldInputDto"][];
            idempotency_key: string;
            rationale: string;
            type: string;
        };
        NewEntityTypeBody: {
            /**
             * Format: int32
             * @description The version this builds on, as the type list gave it. Leave it out for a new type.
             */
            base_version?: number | null;
            idempotency_key: string;
            name: string;
            rationale: string;
            schema: Record<string, never>;
        };
        NewPageBody: {
            /** @description Markdown, with `[[wiki links]]` and `{{claim:<id>}}` references. */
            body: string;
            idempotency_key: string;
            rationale: string;
            /** @description Left out, it's made from the title. */
            slug?: string | null;
            title: string;
        };
        NewPrincipalBody: {
            kind: components["schemas"]["PrincipalKindDto"];
            name: string;
            server_admin?: boolean;
        };
        NewTokenBody: {
            /** Format: int64 */
            expires_at_ms?: number | null;
            grants?: components["schemas"]["GrantDto"][];
            name: string;
            /** @description Whose token it is. The default is the caller. */
            principal_id?: string | null;
        };
        OutgoingLinkDto: {
            block_id?: string | null;
            edge_id: string;
            /** @description `wiki_link` or `claim_ref`. */
            kind: string;
            /** @description Whether the target exists. */
            resolved: boolean;
            /** @description The slug or claim id as written. */
            target: string;
            to_page?: string | null;
        };
        OutgoingLinkPage: {
            items: components["schemas"]["OutgoingLinkDto"][];
            next_cursor?: string | null;
        };
        /** @description What a page's `{{claim:<id>}}` shows now, whichever revision of the page is being read. */
        PageClaimDto: {
            citations: components["schemas"]["FieldEvidenceDto"][];
            /** @description The id the page names. */
            claim_id: string;
            /** @description The claim that stands now: the one named, or what replaced it. Absent when missing. */
            current_claim_id?: string | null;
            /** @description `verified`, `asserted`, `inferred` or `disputed`. */
            epistemic?: string | null;
            predicate?: string | null;
            /** @description `current`, `superseded` or `missing`. */
            status: string;
            subject?: string | null;
            value?: Record<string, never> | null;
        };
        PageDeleteBody: {
            /** Format: int32 */
            base_version: number;
            idempotency_key: string;
            rationale: string;
        };
        PageDetailDto: {
            blocks: components["schemas"]["BlockDto"][];
            /** @description Canonical Markdown: exactly its blocks, joined. */
            body: string;
            claims: components["schemas"]["PageClaimDto"][];
            creator: string;
            /**
             * Format: int32
             * @description The version an edit bases itself on.
             */
            current_revision: number;
            current_slug: string;
            id: string;
            redirects: string[];
            /**
             * Format: int32
             * @description The revision this one restored, on a revert.
             */
            restored_from?: number | null;
            /**
             * Format: int32
             * @description The revision shown.
             */
            revision: number;
            /** @description The slug at the revision shown. */
            slug: string;
            title: string;
            /** Format: int64 */
            updated_at_ms: number;
        };
        PageDiffDto: {
            blocks: components["schemas"]["BlockChangeDto"][];
            /** Format: int32 */
            from: number;
            slug?: string[] | null;
            /** @description Old and new, if it changed. */
            title?: string[] | null;
            /** Format: int32 */
            to: number;
        };
        PageEditBody: {
            /**
             * Format: int32
             * @description The version this edit was made from. If the page has moved on, the answer is a conflict with its current state.
             */
            base_version: number;
            body: string;
            idempotency_key: string;
            rationale: string;
        };
        PageFactsDto: {
            /** Format: int32 */
            height: number;
            /** Format: int32 */
            index: number;
            /** Format: int32 */
            rotation: number;
            /** Format: int32 */
            width: number;
        };
        PageImportBody: {
            /**
             * Format: int32
             * @description Needed to edit the page already holding its slug.
             */
            base_version?: number | null;
            idempotency_key: string;
            /** @description A page as export gives it: front matter with a JSON-quoted `title`, and optionally `slug` and `redirects`, then the Markdown. Without front matter, its first heading is the title. */
            markdown: string;
            rationale: string;
        };
        PageImportResult: {
            /** @description False when it edited an existing page. */
            created: boolean;
            page_id: string;
            result: components["schemas"]["ProposalResult"];
        };
        PageListDto: {
            allowed_dpi: number[];
            /** Format: int32 */
            page_count: number;
            pages: components["schemas"]["PageFactsDto"][];
            rendition_id: string;
        };
        PageProposalResult: {
            page_id: string;
            result: components["schemas"]["ProposalResult"];
        };
        PageRedirectDto: {
            current_slug: string;
            page_id: string;
            /** @description The slug asked for is an old one. */
            redirected: boolean;
            slug: string;
        };
        PageRenameBody: {
            /** Format: int32 */
            base_version: number;
            idempotency_key: string;
            rationale: string;
            /** @description Left out, it's made from the title. The old slug keeps leading here. */
            slug?: string | null;
            title: string;
        };
        PageRevertBody: {
            /** Format: int32 */
            base_version: number;
            idempotency_key: string;
            rationale: string;
            /**
             * Format: int32
             * @description The revision to make current again, as a new revision.
             */
            revision: number;
        };
        PageSummaryDto: {
            creator: string;
            id: string;
            /** @description Old slugs, oldest first, that still lead here. */
            redirects: string[];
            slug: string;
            title: string;
            /** Format: int64 */
            updated_at_ms: number;
            /**
             * Format: int32
             * @description The version an edit bases itself on, which is also the newest revision.
             */
            version: number;
        };
        PageSummaryPage: {
            items: components["schemas"]["PageSummaryDto"][];
            next_cursor?: string | null;
        };
        ParticipantDto: {
            /** @description The Person a committed proposal links them to. */
            entity_id?: string | null;
            /** @description Stable, so a link can follow them. The server gives one to a participant sent without it. */
            id: string;
            name: string;
            /** @description A transcript's label for them, when someone knows it. */
            speaker?: string | null;
        };
        ParticipantInput: {
            /** @description Leave out for a new participant. */
            id?: string | null;
            name: string;
            speaker?: string | null;
        };
        /** @description Proposes that a participant is this Person. */
        ParticipantLinkBody: {
            entity_id: string;
            idempotency_key: string;
            rationale: string;
        };
        PlantBody: {
            /** @description Kinds to plant, and every kind if left out. */
            kinds?: string[] | null;
            /** Format: int32 */
            per_kind: number;
            /** @description A decimal number, as text. */
            seed: string;
        };
        PlantedCountDto: {
            kind: string;
            /** Format: int64 */
            planted: number;
        };
        PlantedDto: {
            kinds: components["schemas"]["PlantedCountDto"][];
        };
        PrincipalDto: {
            /** Format: int64 */
            created_at_ms: number;
            created_by: string;
            disabled: boolean;
            id: string;
            kind: components["schemas"]["PrincipalKindDto"];
            name: string;
            server_admin: boolean;
        };
        /** @enum {string} */
        PrincipalKindDto: "human" | "agent";
        PrincipalPage: {
            items: components["schemas"]["PrincipalDto"][];
            next_cursor?: string | null;
        };
        /** @description What a worker last said about a job. In memory only, so it has no id and no resume. */
        ProgressFrame: {
            /** Format: int64 */
            done: number;
            job_id: string;
            source_id?: string | null;
            stage: string;
            /** Format: int64 */
            time_ms: number;
            /** Format: int64 */
            total?: number | null;
        };
        ProposalBody: {
            idempotency_key: string;
            /** @description The ops, as defined by `memex_types::Op`. */
            ops: Record<string, never>[];
            rationale: string;
        };
        ProposalResult: {
            changeset?: components["schemas"]["ChangeSetDto"] | null;
            contradictions: components["schemas"]["ContradictionDto"][];
            dry_run: boolean;
            replayed: boolean;
            risk: string;
            validators: Record<string, never>[];
        };
        QueueCountDto: {
            /** Format: int64 */
            count: number;
            extractor: string;
            item_type: string;
            prompt_version: string;
            /** @description Absent for items with no verdict yet. */
            verdict?: string | null;
        };
        QueueCountsDto: {
            groups: components["schemas"]["QueueCountDto"][];
            /**
             * Format: int64
             * @description Pending items. One with several types counts once here, and once under each in `groups`.
             */
            pending: number;
        };
        ReanchorFailureDto: {
            assertion_id: string;
            evidence_id: string;
            quote: string;
            segment_id: string;
        };
        ReanchorFailurePage: {
            items: components["schemas"]["ReanchorFailureDto"][];
            next_cursor?: string | null;
        };
        ReasonBody: {
            reason?: string | null;
        };
        /** @description What planted errors in this vault came to. Keep the two groups apart: the deterministic one is measured, and the LLM one means something only when `llm_evidence` is `model`. */
        RecallReportDto: {
            /** Format: int64 */
            clean_audited: number;
            deterministic: components["schemas"]["GroupRecallDto"];
            /** Format: int64 */
            false_positives_deterministic: number;
            /** Format: int64 */
            false_positives_llm: number;
            kinds: components["schemas"]["KindRecallDto"][];
            /** @description `none`, `scripted` or `model`. Scripted answers prove the mechanism and say nothing about a model. */
            llm_evidence: string;
            llm_models: string[];
            llm_only: components["schemas"]["GroupRecallDto"];
            /** Format: int64 */
            overall_caught: number;
            /** Format: int64 */
            overall_planted: number;
            /** Format: double */
            overall_recall?: number | null;
            /** Format: double */
            target: number;
            /** @description Null until it can be judged: every kind planted, with answers from a real model. */
            target_met?: boolean | null;
            /** @description Too few planted errors to trust the figure. */
            too_small: boolean;
        };
        RectDto: {
            /** Format: int32 */
            page: number;
            /** Format: int32 */
            x0: number;
            /** Format: int32 */
            x1: number;
            /** Format: int32 */
            y0: number;
            /** Format: int32 */
            y1: number;
        };
        RedirectDto: {
            /** @description The ids passed on the way there, the asked one first. */
            chain: string[];
            /** @description The entity this id leads to now, which is itself unless it has been merged. */
            current_id: string;
            id: string;
        };
        RejectedItemDto: {
            quote: string;
            reason: string;
            segment: string;
        };
        RenditionDetail: {
            canonical_text: string;
            rendition: components["schemas"]["RenditionDto"];
            /** @description The word table and page facts. Absent for plain text. */
            word_table: Record<string, never>;
        };
        RenditionDto: {
            /** Format: int64 */
            created_at_ms: number;
            id: string;
            /** Format: int32 */
            page_count: number;
            /** @description How each page was read: `route`, mean OCR confidence in permille, and `flags`. */
            pages: Record<string, never>[];
            parser: string;
            /** Format: int32 */
            pipeline_version: number;
            /** Format: int32 */
            segment_count: number;
            source_id: string;
            superseded_by?: string | null;
        };
        RenditionFact: {
            /** Format: int64 */
            created_at_ms: number;
            id: string;
            /** Format: int32 */
            page_count: number;
            /** Format: int32 */
            segment_count: number;
            source_id?: string | null;
            superseded: boolean;
        };
        RenditionList: {
            items: components["schemas"]["RenditionDto"][];
        };
        ReplacesDto: {
            assertion_id: string;
            /**
             * Format: int32
             * @description The claim's version when it was read.
             */
            version: number;
        };
        ResolveResult: {
            /** @description Exact matches first. Choosing among them is the caller's decision. */
            candidates: components["schemas"]["CandidateDto"][];
            /** @description `lexical`, if only exact matches could be looked for. */
            degraded: string[];
        };
        /** @description Fetch a new snapshot, then carry on from here. Also the frame's SSE id. */
        ResyncFrame: {
            /**
             * Format: int64
             * @description The position this stream continues from. -1 is before the first event.
             */
            head: number;
            /** @description `overflow`, `unknown_position`, `log_gap` or `erased`. */
            reason: string;
        };
        ReviewDetailDto: {
            /** @description Why auto-commit wouldn't take this even if earned. */
            auto_commit_block?: string | null;
            changeset: components["schemas"]["ChangeSetDto"];
            /** @description Each type's standing under this proposal's extractor and prompt. A type with no samples has no entry. */
            combinations: components["schemas"]["AutoCommitDto"][];
            evidence: components["schemas"]["ReviewEvidenceDto"][];
            /** @description Only a person can approve this. */
            needs_a_person: boolean;
            verdict?: components["schemas"]["ReviewVerdictDto"] | null;
        };
        ReviewEvidenceDto: {
            claim_id: string;
            /** @description What the pipeline flagged on these pages, such as hidden text. */
            page_flags: Record<string, never>[];
            /** Format: int32 */
            page_from?: number | null;
            /** Format: int32 */
            page_to?: number | null;
            quote: string;
            rendition_id?: string | null;
            segment_id: string;
            source_id?: string | null;
            source_name?: string | null;
            /** @description Its document was processed again, so this can't be approved as it stands. */
            superseded: boolean;
            trust_tier?: string | null;
        };
        ReviewItemDto: {
            changeset: components["schemas"]["ChangeSetDto"];
            verdict?: components["schemas"]["ReviewVerdictDto"] | null;
        };
        ReviewItemPage: {
            items: components["schemas"]["ReviewItemDto"][];
            next_cursor?: string | null;
        };
        /** @description The reviewer agent's opinion. It's advice, and it decides nothing. */
        ReviewVerdictDto: {
            /** Format: int32 */
            attempts: number;
            /** Format: int64 */
            created_at_ms: number;
            model: string;
            prompt_hash: string;
            /** @description The model's own words about untrusted text, so show them as plain text. */
            reason: string;
            tokens: components["schemas"]["TokenUsageDto"];
            /** @description `approve`, `reject` or `abstain`. */
            verdict: string;
        };
        RevisionDto: {
            changeset_id: string;
            /** Format: int64 */
            created_at_ms: number;
            creator: string;
            /** Format: int32 */
            restored_from?: number | null;
            /** Format: int32 */
            revision: number;
            slug: string;
            title: string;
        };
        RevisionPage: {
            items: components["schemas"]["RevisionDto"][];
            next_cursor?: string | null;
        };
        /** @description AG-UI's `RunAgentInput`. Only the newest user message is read: a thread's history is what the server stored, so a client can't put words in the agent's mouth, and `tools`, `context` and `state` are accepted and ignored. */
        RunAgentBody: {
            context?: Record<string, never>[];
            forwardedProps?: Record<string, never>;
            messages: Record<string, never>[];
            runId: string;
            state?: Record<string, never>;
            threadId: string;
            tools?: Record<string, never>[];
        };
        /** @description A suggestion, never a decision: these entities share an identity value or alias. Merging them is a proposal, which a person approves. */
        SameAsDto: {
            entity_ids: string[];
            field: string;
            /** @description The shared value, folded. */
            value: string;
        };
        SameAsPage: {
            items: components["schemas"]["SameAsDto"][];
            next_cursor?: string | null;
        };
        /** @enum {string} */
        ScopeDto: "read" | "propose" | "admin";
        SearchDebugDto: {
            /** Format: float */
            best_vector_score?: number | null;
            candidates_after_fence: number;
            candidates_before_fence: number;
            dropped_terms: number;
            /** @description Candidates that following Edges added. */
            expanded: number;
            lexical_terms: number;
            /** Format: float */
            lexical_weight: number;
            signals: components["schemas"]["SignalScoresDto"][];
            /** Format: float */
            term_coverage?: number | null;
            timings: components["schemas"]["StageTimingDto"][];
            /** Format: float */
            vector_weight: number;
        };
        SegmentDto: {
            /** Format: int64 */
            end: number;
            heading?: string | null;
            id: string;
            language?: string | null;
            /** Format: int32 */
            page_from?: number | null;
            /** Format: int32 */
            page_to?: number | null;
            rendition_id?: string | null;
            source_id: string;
            speaker?: string | null;
            /** Format: int64 */
            start: number;
            /** Format: int64 */
            t_end_ms?: number | null;
            /**
             * Format: int64
             * @description Where it plays, when it comes from a recording.
             */
            t_start_ms?: number | null;
            text: string;
        };
        SegmentPage: {
            items: components["schemas"]["SegmentDto"][];
            next_cursor?: string | null;
        };
        SignalScoresDto: {
            /** Format: float */
            fused: number;
            id: string;
            lexical_rank?: number | null;
            /** Format: float */
            lexical_score?: number | null;
            /** Format: float */
            rerank_score?: number | null;
            vector_rank?: number | null;
            /** Format: float */
            vector_score?: number | null;
        };
        SourceDto: {
            created_by: string;
            id: string;
            logical_id: string;
            media_type: string;
            metadata: Record<string, never>;
            name: string;
            /** Format: int64 */
            recorded_at_ms: number;
            sha256: string;
            /** Format: int64 */
            size: number;
            trust_tier: string;
            /**
             * Format: int32
             * @description What optimistic edits base themselves on.
             */
            version: number;
            /** Format: int32 */
            version_no: number;
        };
        SourceFact: {
            id: string;
            logical_id?: string | null;
            /** Format: int64 */
            size: number;
            /** Format: int32 */
            version_no: number;
        };
        SourcePage: {
            items: components["schemas"]["SourceDto"][];
            next_cursor?: string | null;
        };
        SourceVersions: {
            items: components["schemas"]["SourceDto"][];
        };
        SplitBody: {
            /** @description The aliases that move to the new entity. */
            aliases?: string[];
            /** Format: int32 */
            base_version: number;
            /** @description Anything else the new entity needs to be valid, such as its own name. */
            fields?: components["schemas"]["FieldInputDto"][];
            idempotency_key: string;
            /** @description The active claims that move to the new entity. */
            moved?: string[];
            rationale: string;
        };
        StageTimingDto: {
            /** Format: double */
            ms: number;
            stage: string;
        };
        /** @description A token's details. The token itself and its hash never appear here. */
        TokenDto: {
            /** Format: int64 */
            created_at_ms: number;
            created_by: string;
            /** Format: int64 */
            expires_at_ms?: number | null;
            grants: components["schemas"]["GrantDto"][];
            id: string;
            /** Format: int64 */
            last_used_at_ms?: number | null;
            name: string;
            principal_id: string;
            /** Format: int64 */
            revoked_at_ms?: number | null;
        };
        TokenPage: {
            items: components["schemas"]["TokenDto"][];
            next_cursor?: string | null;
        };
        TokenUsageDto: {
            /** Format: int64 */
            input_tokens: number;
            /** Format: int64 */
            output_tokens: number;
            /** Format: int64 */
            thinking_tokens: number;
        };
        TranscriptDto: {
            /** Format: int64 */
            duration_ms: number;
            next_cursor?: string | null;
            parser: string;
            parts: components["schemas"]["TranscriptPartDto"][];
            rendition_id: string;
        };
        /** @description A stretch of a transcript: a turn, cut to the window asked for. Its range is the same a claim citing these words gets. */
        TranscriptPartDto: {
            /** Format: int64 */
            end: number;
            /** @description Who was speaking, when that's known and one person spoke all of it. */
            speaker?: string | null;
            /**
             * Format: int64
             * @description UTF-8 byte offsets into the rendition's canonical text.
             */
            start: number;
            /** Format: int64 */
            t_end_ms: number;
            /** Format: int64 */
            t_start_ms: number;
            text: string;
            words?: components["schemas"]["TranscriptWordDto"][] | null;
        };
        TranscriptWordDto: {
            /** Format: int64 */
            end_ms: number;
            /**
             * Format: int64
             * @description The table's own times, with no padding.
             */
            start_ms: number;
            text: string;
        };
        /** @description What to run. Anything left out comes from the schedule. The same seed on the same state gives the same sample. */
        TriggerAuditBody: {
            /** Format: int64 */
            run_token_budget?: number | null;
            /** Format: int32 */
            sample_size?: number | null;
            /** @description A decimal number, as text, since it can be too large for a JSON number to carry safely. */
            seed?: string | null;
        };
        TypePreviewBody: {
            schema: Record<string, never>;
        };
        /** @description What submitting this schema would do for this caller. Nothing is written. */
        TypePreviewDto: {
            /** @description `new`, `additive` or `breaking`. */
            change: string;
            /** Format: int32 */
            current_version?: number | null;
            /** @description `auto_commit`, `review` or `reject`. */
            decision: string;
            /** Format: int32 */
            next_version: number;
            /** @description Why a submission would be refused. Absent if it's acceptable. */
            problem?: string | null;
            reasons: string[];
        };
        UnresolvedLinkDto: {
            block_id?: string | null;
            edge_id: string;
            from_page: string;
            /** @description `wiki_link` or `claim_ref`. */
            kind: string;
            /** @description The slug or claim id that leads nowhere. */
            target: string;
        };
        UnresolvedLinkPage: {
            items: components["schemas"]["UnresolvedLinkDto"][];
            next_cursor?: string | null;
        };
        UploadLimitsDto: {
            allowed_media_types: string[];
            /** Format: int64 */
            max_bytes: number;
        };
        UploadResult: {
            /** @description These bytes were already here, so nothing new was created. */
            existing: boolean;
            job_id?: string | null;
            source: components["schemas"]["SourceDto"];
        };
        VaultDto: {
            /** Format: int64 */
            created_at_ms: number;
            created_by: string;
            id: string;
            name: string;
        };
        VaultFindingDto: {
            check: string;
            detail: string;
            /** @description `pass`, `fail` or `inconclusive`. */
            status: string;
        };
        VaultPage: {
            items: components["schemas"]["VaultDto"][];
            next_cursor?: string | null;
        };
        VersionDto: {
            version: string;
        };
        WhoAmI: {
            grants: components["schemas"]["GrantDto"][];
            principal: components["schemas"]["PrincipalDto"];
            token_id: string;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export type operations = Record<string, never>;
