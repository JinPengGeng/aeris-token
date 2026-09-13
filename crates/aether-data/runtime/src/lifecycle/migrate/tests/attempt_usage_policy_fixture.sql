-- Only consumed by an isolated migration test database.
CREATE TABLE public.request_fund_reservations (
    reservation_token varchar(128) PRIMARY KEY,
    attempt_id uuid
);
CREATE TABLE public.usage_cost_reservations (
    reservation_token varchar(128) PRIMARY KEY
);
INSERT INTO public.request_fund_reservations VALUES ('legacy', NULL);
INSERT INTO public.usage_cost_reservations VALUES ('legacy');

-- Constraint names are not globally unique, even within the same schema.
CREATE TABLE public.unrelated_policy (
    value integer CONSTRAINT request_fund_reservations_usage_policy_check CHECK (value > 0)
);
CREATE SCHEMA unrelated;
CREATE TABLE unrelated.quota (
    value integer CONSTRAINT usage_cost_reservations_attempt_token_check CHECK (value > 0),
    other_value integer CONSTRAINT usage_cost_reservations_attempt_token_fkey CHECK (other_value > 0)
);
