//! Existing wallet mutation transactions, with an optional durable audit intent.
use super::*;

impl SqlxWalletRepository {
    pub(super) async fn adjust_wallet_balance_inner(
        &self,
        input: AdjustWalletBalanceInput,
        audit: Option<&aether_data_contracts::repository::audit::CreateAdminAuditLog>,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminWalletTransaction)>, DataLayerError> {
        if !input.amount_usd.is_finite() || input.amount_usd == 0.0 {
            return Err(DataLayerError::InvalidInput(
                "adjustment amount must be finite and non-zero".to_string(),
            ));
        }
        let intent = prepare_intent(audit)?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(row) = sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(None);
                    };

                    let before_recharge: f64 = row_get(&row, "balance")?;
                    let before_gift: f64 = row_get(&row, "gift_balance")?;
                    let before_total = before_recharge + before_gift;
                    let before_total_adjusted: f64 = row_get(&row, "total_adjusted")?;
                    if !before_recharge.is_finite()
                        || !before_gift.is_finite()
                        || !before_total.is_finite()
                        || !before_total_adjusted.is_finite()
                    {
                        return Err(DataLayerError::UnexpectedValue(
                            "wallet balance is invalid".to_string(),
                        ));
                    }
                    let mut after_recharge = before_recharge;
                    let mut after_gift = before_gift;

                    if input.amount_usd > 0.0 {
                        if input.balance_type.eq_ignore_ascii_case("gift") {
                            after_gift += input.amount_usd;
                        } else {
                            after_recharge += input.amount_usd;
                        }
                    } else {
                        // Apply the same NUMERIC(20,8) scale as the adjustment ledger,
                        // then subtract units so an exact hold boundary stays exact.
                        let debit_usd: f64 = sqlx::query_scalar(
                            "SELECT (-$1::double precision)::numeric(20,8)::double precision",
                        )
                        .bind(input.amount_usd)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        let mut remaining = request_funds_available_units(debit_usd)?;
                        let consume_positive_bucket =
                            |balance: &mut f64,
                             to_consume: &mut u64|
                             -> Result<(), DataLayerError> {
                                if *to_consume == 0 {
                                    return Ok(());
                                }
                                let available = request_funds_available_units((*balance).max(0.0))?;
                                let consumed = available.min(*to_consume);
                                if consumed > 0 {
                                    *balance = request_funds_usd(available - consumed);
                                }
                                *to_consume -= consumed;
                                Ok(())
                            };
                        if input.balance_type.eq_ignore_ascii_case("gift") {
                            consume_positive_bucket(&mut after_gift, &mut remaining)?;
                            consume_positive_bucket(&mut after_recharge, &mut remaining)?;
                        } else {
                            consume_positive_bucket(&mut after_recharge, &mut remaining)?;
                            consume_positive_bucket(&mut after_gift, &mut remaining)?;
                        }
                        if remaining > 0 {
                            // INTENTIONAL (issue #208): administrator adjustments may
                            // drive the recharge balance negative. The excess is
                            // recorded as recharge debt after both positive buckets
                            // are consumed, mirroring usage-settlement overdraft so a
                            // later recharge can restore the balance. This is a
                            // deliberate operator-facing capability, not an oversight;
                            // do not "fix" it by rejecting the adjustment.
                            after_recharge -= request_funds_usd(remaining);
                        }
                    }
                    let after_total = after_recharge + after_gift;
                    let after_total_adjusted = before_total_adjusted + input.amount_usd;
                    crate::settlement::funding::ensure_wallet_holds_preserved(
                        tx,
                        &input.wallet_id,
                        after_recharge,
                        after_gift,
                    )
                    .await?;
                    if !after_recharge.is_finite()
                        || !after_gift.is_finite()
                        || !after_total.is_finite()
                        || !after_total_adjusted.is_finite()
                    {
                        return Err(DataLayerError::UnexpectedValue(
                            "wallet balance overflow during admin adjustment".to_string(),
                        ));
                    }

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  gift_balance = $3,
  total_adjusted = total_adjusted + $4,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(after_recharge)
                    .bind(after_gift)
                    .bind(input.amount_usd)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let transaction_id = Uuid::new_v4().to_string();
                    let created_at = Utc::now().timestamp().max(0) as u64;
                    let description = input
                        .description
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("管理员调账")
                        .to_string();
                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'adjust',
  'adjust_admin',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'admin_action',
  $10,
  $11,
  $12,
  NOW()
)
                        "#,
                    )
                    .bind(&transaction_id)
                    .bind(&input.wallet_id)
                    .bind(input.amount_usd)
                    .bind(before_total)
                    .bind(after_total)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(after_gift)
                    .bind(&input.wallet_id)
                    .bind(input.operator_id.as_deref())
                    .bind(&description)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    enqueue_intent(tx, intent).await?;

                    Ok(Some((
                        wallet,
                        StoredAdminWalletTransaction {
                            id: transaction_id,
                            wallet_id: input.wallet_id,
                            category: "adjust".to_string(),
                            reason_code: "adjust_admin".to_string(),
                            amount: input.amount_usd,
                            balance_before: before_total,
                            balance_after: after_total,
                            recharge_balance_before: before_recharge,
                            recharge_balance_after: after_recharge,
                            gift_balance_before: before_gift,
                            gift_balance_after: after_gift,
                            link_type: Some("admin_action".to_string()),
                            link_id: Some(row_get(&wallet_row, "id")?),
                            operator_id: input.operator_id,
                            operator_name: None,
                            operator_email: None,
                            description: Some(description),
                            created_at_unix_ms: Some(created_at),
                        },
                    )))
                })
            })
            .await
    }

    pub(super) async fn create_manual_wallet_recharge_inner(
        &self,
        mut input: CreateManualWalletRechargeInput,
        audit: Option<&aether_data_contracts::repository::audit::CreateAdminAuditLog>,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, DataLayerError> {
        input.payment_method = canonicalize_payment_method(&input.payment_method)
            .map_err(DataLayerError::InvalidInput)?;
        if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
            return Err(DataLayerError::InvalidInput(
                "manual recharge amount must be finite and positive".to_string(),
            ));
        }
        let intent = prepare_intent(audit)?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(None);
                    };

                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let before_total_recharged: f64 = row_get(&wallet_row, "total_recharged")?;
                    let (after_recharge, after_total_recharged) = validate_manual_wallet_recharge(
                        input.amount_usd,
                        before_recharge,
                        before_gift,
                        before_total_recharged,
                    )
                    .map_err(DataLayerError::InvalidInput)?;
                    let user_id: Option<String> = row_get(&wallet_row, "user_id")?;
                    let gateway_response = serde_json::json!({
                        "source": "manual",
                        "operator_id": input.operator_id,
                        "description": input.description,
                    });

                    let order_id = Uuid::new_v4().to_string();
                    sqlx::query(
                        r#"
INSERT INTO payment_orders (
  id,
  order_no,
  wallet_id,
  user_id,
  amount_usd,
  refunded_amount_usd,
  refundable_amount_usd,
  payment_method,
  status,
  gateway_response,
  created_at,
  paid_at,
  credited_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  0,
  $5,
  $6,
  'credited',
  $7,
  NOW(),
  NOW(),
  NOW()
)
                        "#,
                    )
                    .bind(&order_id)
                    .bind(&input.order_no)
                    .bind(&input.wallet_id)
                    .bind(user_id.as_deref())
                    .bind(input.amount_usd)
                    .bind(&input.payment_method)
                    .bind(&gateway_response)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_recharged = $3,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(after_recharge)
                    .bind(after_total_recharged)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let reason_code = if matches!(
                        input.payment_method.as_str(),
                        "card_code" | "gift_code" | "card_recharge"
                    ) {
                        "topup_card_code"
                    } else {
                        "topup_admin_manual"
                    };
                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'recharge',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $9,
  'payment_order',
  $10,
  $11,
  $12,
  NOW()
)
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&input.wallet_id)
                    .bind(reason_code)
                    .bind(input.amount_usd)
                    .bind(before_recharge + before_gift)
                    .bind(after_recharge + before_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(&order_id)
                    .bind(input.operator_id.as_deref())
                    .bind(
                        input
                            .description
                            .as_deref()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or("管理员手动充值"),
                    )
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let order_row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let order = map_admin_payment_order_row(&order_row)?;
                    enqueue_intent(tx, intent).await?;
                    Ok(Some((wallet, order)))
                })
            })
            .await
    }
}

fn prepare_intent(
    audit: Option<&aether_data_contracts::repository::audit::CreateAdminAuditLog>,
) -> Result<Option<(String, serde_json::Value)>, DataLayerError> {
    audit
        .map(|record| {
            record.validate()?;
            let payload = serde_json::to_value(record).map_err(|_| {
                DataLayerError::InvalidInput(
                    "invalid wallet administrator audit intent".to_string(),
                )
            })?;
            Ok((record.id.clone(), payload))
        })
        .transpose()
}

async fn enqueue_intent(
    tx: &mut crate::PostgresTransaction,
    intent: Option<(String, serde_json::Value)>,
) -> Result<(), DataLayerError> {
    if let Some((id, payload)) = intent {
        // Duplicate intent IDs roll back the complete business transaction.
        // Audit delivery retries never invoke these wallet mutation methods.
        sqlx::query("INSERT INTO admin_audit_delivery(event_id,payload) VALUES($1,$2)")
            .bind(id)
            .bind(payload)
            .execute(&mut **tx)
            .await
            .map_postgres_err()?;
    }
    Ok(())
}
