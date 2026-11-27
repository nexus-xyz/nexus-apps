//! Bounded incremental completion metadata; old event payloads are never revisited.
use super::*;
use crate::event::TurnStatus;

fn end() -> WbEvent {
    WbEvent::TurnEnd {
        turn_id: "actual-turn".into(),
        status: TurnStatus::Completed,
        cost_usd: None,
        duration_ms: 0,
        error: None,
        cursor: None,
    }
}
#[test]
fn completion_pages_advance_across_backlog_without_skipping_terminal_events() -> Result<()> {
    let store = Store::in_memory()?;
    for seq in 1..=600 {
        let event = if seq == 257 || seq == 513 {
            end()
        } else {
            WbEvent::Notice {
                text: format!("event {seq}"),
            }
        };
        store.append("original", seq, &event)?;
    }
    store.append("other", 1, &end())?;
    let reader = store.reader();
    assert_eq!(reader.latest_event_seq("original")?, 600);
    assert_eq!(
        reader.turn_end_page("original", 0)?,
        TurnEndPage {
            through_seq: 256,
            turn_end: None,
            more: true
        }
    );
    assert_eq!(
        reader.turn_end_page("original", 256)?,
        TurnEndPage {
            through_seq: 512,
            turn_end: Some(257),
            more: true
        }
    );
    assert_eq!(
        reader.turn_end_page("original", 512)?,
        TurnEndPage {
            through_seq: 600,
            turn_end: Some(513),
            more: false
        }
    );
    assert_eq!(reader.turn_end_page("other", 0)?.turn_end, Some(1));
    Ok(())
}
#[test]
fn unchanged_and_incremental_pages_do_not_parse_consumed_payloads_again() -> Result<()> {
    let store = Store::in_memory()?;
    store.append("original", 1, &end())?;
    let reader = store.reader();
    assert_eq!(reader.turn_end_page("original", 0)?.through_seq, 1);
    let corrupt = "update events set body='not json' where thread_id='original' and seq=1";
    // The production lasting-event partial index validates JSON during UPDATE, before our read query runs.
    let rejected = store
        .db
        .execute(corrupt, [])
        .expect_err("the lasting-event index must reject malformed JSON");
    assert!(rejected.to_string().contains("malformed JSON"));
    // Remove only that unrelated index in this private fault-injection database. The completion query still
    // uses the real primary-key cursor and page limit; any reinspection of the consumed body below must fail.
    store
        .db
        .execute_batch("drop index events_lasting_rewind")
        .map_err(sql)?;
    store.db.execute(corrupt, []).map_err(sql)?;
    assert_eq!(
        reader.turn_end_page("original", 1)?,
        TurnEndPage {
            through_seq: 1,
            turn_end: None,
            more: false
        }
    );
    store.append(
        "original",
        2,
        &WbEvent::Notice {
            text: "new ordinary event".into(),
        },
    )?;
    store.append("original", 3, &end())?;
    assert_eq!(
        reader.turn_end_page("original", 1)?,
        TurnEndPage {
            through_seq: 3,
            turn_end: Some(3),
            more: false
        }
    );
    Ok(())
}
#[test]
fn missing_and_truncated_logs_return_a_safe_cursor_without_payload_work() -> Result<()> {
    let store = Store::in_memory()?;
    let reader = store.reader();
    assert_eq!(
        reader.turn_end_page("missing", 0)?,
        TurnEndPage {
            through_seq: 0,
            turn_end: None,
            more: false
        }
    );
    store.append("original", 1, &end())?;
    store.append("original", 2, &end())?;
    assert_eq!(reader.latest_event_seq("original")?, 2);
    store
        .db
        .execute("delete from events where thread_id='original' and seq>1", [])
        .map_err(sql)?;
    assert_eq!(
        reader.turn_end_page("original", 2)?,
        TurnEndPage {
            through_seq: 1,
            turn_end: None,
            more: false
        }
    );
    Ok(())
}
