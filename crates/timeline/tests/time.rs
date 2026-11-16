//! Exact time: rates, timecode both ways, the verb time parser, and its refusals.

use k2_mpc_timeline::time::Sign;
use k2_mpc_timeline::{parse_time, Rate, Snap, Time, TimeError, Timecode};

fn r(num: u32, den: u32) -> Rate {
    Rate::new(num, den).unwrap()
}

fn t(num: i64, den: u64) -> Time {
    Time::new(num, den).unwrap()
}

#[test]
fn floating_rates_map_to_the_rate_they_name() {
    let rows: &[(f64, (u32, u32))] = &[
        (23.976, (24000, 1001)),
        (23.976023976023978, (24000, 1001)),
        (24.0, (24, 1)),
        (25.0, (25, 1)),
        (29.97, (30000, 1001)),
        (29.97002997002997, (30000, 1001)),
        (30.0, (30, 1)),
        (47.952, (48000, 1001)),
        (50.0, (50, 1)),
        (59.94, (60000, 1001)),
        (119.88, (120000, 1001)),
        (12.5, (25, 2)),
        (48000.0, (48000, 1)),
    ];
    for &(fps, (n, d)) in rows {
        let got = Rate::from_f64(fps).unwrap_or_else(|e| panic!("{fps}: {e}"));
        assert_eq!((got.num(), got.den()), (n, d), "{fps}");
    }
    for bad in [
        0.0,
        -24.0,
        f64::NAN,
        f64::INFINITY,
        std::f64::consts::SQRT_2,
    ] {
        assert_eq!(Rate::from_f64(bad), Err(TimeError::BadRate), "{bad}");
    }
}

#[test]
fn rates_reduce_and_refuse_zero() {
    assert_eq!(r(60, 2), r(30, 1));
    assert_eq!(Rate::new(0, 1), Err(TimeError::BadRate));
    assert_eq!(Rate::new(1, 0), Err(TimeError::BadRate));
    assert_eq!(Rate::ntsc(30).unwrap(), r(30000, 1001));
    assert_eq!(
        Rate::from_frame_duration(t(1001, 30000)).unwrap(),
        r(30000, 1001)
    );
    assert_eq!(
        Rate::from_frame_duration(Time::ZERO),
        Err(TimeError::BadRate)
    );
    assert_eq!(r(30000, 1001).frame(), t(1001, 30000));
}

#[test]
fn non_drop_timecode_round_trips_every_frame_of_two_hours_at_25() {
    let rate = r(25, 1);
    for f in (0..25 * 3600 * 2).step_by(7) {
        let tc = Timecode::from_frames(f, rate, false).unwrap();
        assert_eq!(tc.to_frames(rate).unwrap(), f);
        let again = Timecode::parse(&tc.to_string()).unwrap();
        assert_eq!(again, tc);
    }
    assert_eq!(
        Timecode::from_frames(25 * 3600 + 25 * 61 + 3, rate, false)
            .unwrap()
            .to_string(),
        "01:01:01:03"
    );
}

#[test]
fn drop_frame_timecode_round_trips_and_skips_the_right_labels() {
    for (rate, d) in [(r(30000, 1001), 2), (r(60000, 1001), 4)] {
        let base = i64::from(rate.timecode_base().unwrap());
        let per_ten = base * 600 - 9 * d;
        for f in 0..per_ten * 7 {
            let tc = Timecode::from_frames(f, rate, true).unwrap();
            assert_eq!(tc.to_frames(rate).unwrap(), f, "{rate} frame {f} {tc}");
            assert!(
                !(tc.minutes % 10 != 0 && tc.seconds == 0 && i64::from(tc.frames) < d),
                "{rate}: frame {f} got a skipped label {tc}"
            );
        }
        // the first minute ends on its last label; the next minute starts past the skipped ones
        let minute = base * 60;
        assert_eq!(
            Timecode::from_frames(minute - 1, rate, true)
                .unwrap()
                .to_string(),
            format!("00:00:59;{:02}", base - 1)
        );
        assert_eq!(
            Timecode::from_frames(minute, rate, true)
                .unwrap()
                .to_string(),
            format!("00:01:00;{d:02}")
        );
        // every tenth minute keeps its first labels
        assert_eq!(
            Timecode::from_frames(per_ten, rate, true)
                .unwrap()
                .to_string(),
            "00:10:00;00"
        );
    }
    let rate = r(30000, 1001);
    assert_eq!(
        Timecode::parse("00:01:00;00").unwrap().to_frames(rate),
        Err(TimeError::BadTimecode {
            why: "drop-frame skips this label"
        })
    );
    assert_eq!(
        Timecode::parse("00:01:00;02").unwrap().to_frames(rate),
        Ok(1800)
    );
    assert_eq!(
        Timecode::parse("01:00:00;00").unwrap().to_frames(rate),
        Ok(107892)
    );
}

#[test]
fn timecode_refusals_are_typed() {
    assert!(matches!(
        Timecode::from_frames(0, r(25, 1), true),
        Err(TimeError::BadTimecode { .. })
    ));
    assert_eq!(
        Timecode::from_frames(0, r(25, 2), false),
        Err(TimeError::NoTimecode { rate: r(25, 2) })
    );
    assert!(matches!(
        Timecode::from_frames(-1, r(25, 1), false),
        Err(TimeError::BadTimecode { .. })
    ));
    assert!(matches!(
        Timecode::parse("00:00:00:25").unwrap().to_frames(r(25, 1)),
        Err(TimeError::BadTimecode { .. })
    ));
    for bad in [
        "00:00:00",
        "00:60:00:00",
        "00:00:60:00",
        "aa:00:00:00",
        "00:00:00:",
        "0:0:0:0:0",
        "01;00:00:00",
    ] {
        assert!(
            matches!(Timecode::parse(bad), Err(TimeError::BadTimecode { .. })),
            "{bad}"
        );
    }
}

#[test]
fn the_verb_parser_reads_each_form() {
    let fps25 = r(25, 1);
    let p = |s: &str| parse_time(s, fps25, Snap::Refuse);
    let one = p("10f").unwrap();
    assert_eq!((one.sign, one.frames, one.time), (None, 10, t(10, 25)));
    let plus = p("+2s").unwrap();
    assert_eq!((plus.sign, plus.frames), (Some(Sign::Plus), 50));
    let minus = p("-10f").unwrap();
    assert_eq!((minus.sign, minus.frames), (Some(Sign::Minus), 10));
    assert_eq!(p("00:00:01:05").unwrap().frames, 30);
    assert_eq!(p("2/5s").unwrap().frames, 10);
    assert_eq!(p("0.04s").unwrap().frames, 1);
    let ntsc = r(24000, 1001);
    assert_eq!(
        parse_time("1001/24000s", ntsc, Snap::Refuse)
            .unwrap()
            .frames,
        1
    );
    assert_eq!(
        parse_time("01:00:00:00", r(24, 1), Snap::Refuse)
            .unwrap()
            .frames,
        86400
    );
}

#[test]
fn off_grid_is_refused_with_the_frames_around_it_unless_asked() {
    let fps25 = r(25, 1);
    assert_eq!(
        parse_time("0.5s", fps25, Snap::Refuse),
        Err(TimeError::OffGrid {
            time: t(1, 2),
            rate: fps25,
            before: t(12, 25),
            after: t(13, 25),
        })
    );
    // a tie goes to the earlier frame
    assert_eq!(parse_time("0.5s", fps25, Snap::Nearest).unwrap().frames, 12);
    assert_eq!(parse_time("0.5s", fps25, Snap::Floor).unwrap().frames, 12);
    assert_eq!(parse_time("0.5s", fps25, Snap::Ceil).unwrap().frames, 13);
    assert_eq!(
        parse_time("0.51s", fps25, Snap::Nearest).unwrap().frames,
        13
    );
    assert!(t(2, 25).on_grid(fps25));
    assert!(!t(1, 50).on_grid(fps25));
    // a negative time between frames floors toward earlier
    assert_eq!(t(-1, 50).to_frames(fps25, Snap::Floor), Ok(-1));
}

#[test]
fn the_verb_parser_refuses_what_it_cannot_read() {
    let fps25 = r(25, 1);
    let p = |s: &str| parse_time(s, fps25, Snap::Refuse);
    assert_eq!(p(""), Err(TimeError::Empty));
    assert_eq!(p("+"), Err(TimeError::Empty));
    for bad in ["12", "1.5x", "f", "--1f", "1e3s", "1/0s", "s1"] {
        assert!(p(bad).is_err(), "{bad}");
    }
    assert!(matches!(p("12"), Err(TimeError::Syntax { .. })));
    assert!(matches!(
        p("00:00:00;00"),
        Err(TimeError::BadTimecode { .. })
    ));
}

#[test]
fn seconds_text_round_trips_exactly() {
    for (n, d) in [
        (0, 1),
        (5, 1),
        (1001, 30000),
        (-3, 2),
        (21602243, 144000),
        (i64::MAX, 1),
        (i64::MIN + 1, 7),
    ] {
        let x = t(n, d);
        assert_eq!(Time::parse_seconds(&x.to_string()).unwrap(), x, "{x}");
    }
    assert_eq!(Time::parse_seconds("2.25s").unwrap(), t(9, 4));
    assert_eq!(Time::parse_seconds(".5s").unwrap(), t(1, 2));
    assert!(Time::parse_seconds("5").is_err());
    assert!(Time::parse_seconds("1/0s").is_err());
    assert!(Time::parse_seconds("1.s").is_err());
    assert!(Time::parse_seconds("s").is_err());
    assert_eq!(
        Time::parse_seconds("99999999999999999999999s"),
        Err(TimeError::Overflow)
    );
}

#[test]
fn overflow_is_refused_never_wrapped() {
    assert_eq!(Time::frames(i64::MAX, r(1, 1000)), Err(TimeError::Overflow));
    assert_eq!(
        t(i64::MIN + 1, 1).checked_sub(t(2, 1)),
        Err(TimeError::Overflow)
    );
    assert_eq!(
        t(1, u64::MAX).checked_add(t(1, u64::MAX - 1)),
        Err(TimeError::Overflow)
    );
    assert_eq!(t(3, 1).checked_scale(1001, 30000).unwrap(), t(1001, 10000));
}
