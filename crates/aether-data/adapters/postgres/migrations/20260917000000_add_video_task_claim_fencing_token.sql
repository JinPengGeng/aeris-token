ALTER TABLE public.video_tasks
    ADD COLUMN IF NOT EXISTS claim_fencing_token BIGINT NOT NULL DEFAULT 0;

ALTER TABLE public.video_tasks
    ADD COLUMN IF NOT EXISTS row_revision BIGINT NOT NULL DEFAULT 1;
