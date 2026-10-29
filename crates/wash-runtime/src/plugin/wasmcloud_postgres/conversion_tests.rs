use super::*;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn round_trip<T: ToSql>(value: PgValue, expected: PgValue, ty: PgType, native: &T) -> TestResult {
    let mut wire = BytesMut::new();
    assert!(matches!(native.to_sql(&ty, &mut wire)?, IsNull::No));
    let mut encoded = BytesMut::new();
    assert!(matches!(
        value.to_sql_checked(&ty, &mut encoded)?,
        IsNull::No
    ));
    assert_eq!(encoded, wire, "encoding {value:?} as {ty}");
    let decoded = PgValue::from_sql(&ty, &wire)?;
    assert_eq!(
        format!("{decoded:?}"),
        format!("{expected:?}"),
        "decoding {ty}"
    );
    Ok(())
}

#[test]
fn scalars_and_arrays_match_postgres_wire_values() -> TestResult {
    macro_rules! check {
        ($variant:ident, $value:expr, $ty:ident) => {
            let value = PgValue::$variant($value);
            round_trip(value.clone(), value, PgType::$ty, &$value)?;
        };
        ($variant:ident, $value:expr, $ty:ident, $native:expr) => {
            let value = PgValue::$variant($value);
            round_trip(value.clone(), value, PgType::$ty, &$native)?;
        };
    }
    check!(Bool, true, BOOL);
    check!(BoolArray, vec![true, false], BOOL_ARRAY);
    check!(Bytea, vec![0_u8, 255], BYTEA);
    check!(ByteaArray, vec![vec![0_u8, 255], vec![]], BYTEA_ARRAY);
    check!(Int2, -123_i16, INT2);
    check!(Int2Array, vec![i16::MIN, i16::MAX], INT2_ARRAY);
    check!(Int4, -123_i32, INT4);
    check!(Int4Array, vec![i32::MIN, i32::MAX], INT4_ARRAY);
    check!(Int8, -123_i64, INT8);
    check!(Int8Array, vec![i64::MIN, i64::MAX], INT8_ARRAY);
    check!(Float4, 1.5_f32.integer_decode(), FLOAT4, 1.5_f32);
    check!(Float8, (-2.5_f64).integer_decode(), FLOAT8, -2.5_f64);
    check!(
        Float4Array,
        vec![1.5_f32.integer_decode(), (-2.5_f32).integer_decode()],
        FLOAT4_ARRAY,
        vec![1.5_f32, -2.5]
    );
    check!(
        Float8Array,
        vec![1.5_f64.integer_decode(), (-2.5_f64).integer_decode()],
        FLOAT8_ARRAY,
        vec![1.5_f64, -2.5]
    );
    check!(Name, "name".to_string(), NAME);
    check!(Text, "héllo".to_string(), TEXT);
    check!(Xml, "<x/>".to_string(), XML);
    check!(TextArray, vec!["héllo".to_string(), "".into()], TEXT_ARRAY);
    check!(NameArray, vec!["one".to_string(), "two".into()], NAME_ARRAY);
    check!(XmlArray, vec!["<x/>".to_string()], XML_ARRAY);
    check!(Varchar, (None, b"hello".to_vec()), VARCHAR, "hello");
    check!(
        VarcharArray,
        vec![(None, b"hello".to_vec())],
        VARCHAR_ARRAY,
        vec!["hello"]
    );
    check!(
        Json,
        "{\"a\":1}".to_string(),
        JSON,
        serde_json::json!({"a":1})
    );
    check!(
        Jsonb,
        "{\"a\":1}".to_string(),
        JSONB,
        serde_json::json!({"a":1})
    );
    check!(
        JsonArray,
        vec!["null".to_string(), "true".into()],
        JSON_ARRAY,
        vec![serde_json::Value::Null, serde_json::json!(true)]
    );
    check!(
        JsonbArray,
        vec!["null".to_string()],
        JSONB_ARRAY,
        vec![serde_json::Value::Null]
    );
    check!(
        Inet,
        "127.0.0.1".to_string(),
        INET,
        "127.0.0.1".parse::<IpAddr>()?
    );
    check!(
        InetArray,
        vec!["::1".to_string()],
        INET_ARRAY,
        vec!["::1".parse::<IpAddr>()?]
    );
    check!(
        Cidr,
        "192.0.2.0/24".to_string(),
        CIDR,
        "192.0.2.0/24".parse::<IpCidr>()?
    );
    check!(
        CidrArray,
        vec!["192.0.2.0/24".to_string()],
        CIDR_ARRAY,
        vec!["192.0.2.0/24".parse::<IpCidr>()?]
    );
    let uuid = Uuid::from_u128(123);
    check!(Uuid, uuid.to_string(), UUID, uuid);
    check!(UuidArray, vec![uuid.to_string()], UUID_ARRAY, vec![uuid]);
    check!(PgLsn, 123_u64, PG_LSN, PgLsn::from(123));
    check!(
        PgLsnArray,
        vec![123_u64, 456],
        PG_LSN_ARRAY,
        vec![PgLsn::from(123), PgLsn::from(456)]
    );
    Ok(())
}

#[test]
fn dates_times_and_geometry_preserve_values() -> TestResult {
    let day = NaiveDate::from_ymd_opt(2024, 2, 29).ok_or("invalid fixture date")?;
    let time = NaiveTime::from_hms_micro_opt(23, 59, 58, 123456).ok_or("invalid fixture time")?;
    let ts = NaiveDateTime::new(day, time);
    for date in [day, NaiveDate::MIN, NaiveDate::MAX] {
        round_trip(
            PgValue::Date(naive_to_date(date)),
            PgValue::Date(naive_to_date(date)),
            PgType::DATE,
            &date,
        )?;
    }
    round_trip(
        PgValue::DateArray(vec![naive_to_date(day)]),
        PgValue::DateArray(vec![naive_to_date(day)]),
        PgType::DATE_ARRAY,
        &vec![day],
    )?;
    round_trip(
        PgValue::Time(naive_to_time(time)),
        PgValue::Time(naive_to_time(time)),
        PgType::TIME,
        &time,
    )?;
    round_trip(
        PgValue::TimeArray(vec![naive_to_time(time)]),
        PgValue::TimeArray(vec![naive_to_time(time)]),
        PgType::TIME_ARRAY,
        &vec![time],
    )?;
    round_trip(
        PgValue::Timestamp(naive_to_timestamp(ts)),
        PgValue::Timestamp(naive_to_timestamp(ts)),
        PgType::TIMESTAMP,
        &ts,
    )?;
    round_trip(
        PgValue::TimestampArray(vec![naive_to_timestamp(ts)]),
        PgValue::TimestampArray(vec![naive_to_timestamp(ts)]),
        PgType::TIMESTAMP_ARRAY,
        &vec![ts],
    )?;
    let utc = ts.and_utc();
    round_trip(
        PgValue::TimestampTz(utc_to_timestamptz(utc)),
        PgValue::TimestampTz(utc_to_timestamptz(utc)),
        PgType::TIMESTAMPTZ,
        &utc,
    )?;
    round_trip(
        PgValue::TimestampTzArray(vec![utc_to_timestamptz(utc)]),
        PgValue::TimestampTzArray(vec![utc_to_timestamptz(utc)]),
        PgType::TIMESTAMPTZ_ARRAY,
        &vec![utc],
    )?;
    let point = Point::new(1.5, -2.5);
    round_trip(
        PgValue::Point(point_to_hashable_f64s(point)),
        PgValue::Point(point_to_hashable_f64s(point)),
        PgType::POINT,
        &point,
    )?;
    round_trip(
        PgValue::PointArray(vec![point_to_hashable_f64s(point)]),
        PgValue::PointArray(vec![point_to_hashable_f64s(point)]),
        PgType::POINT_ARRAY,
        &vec![point],
    )?;
    let rect = Rect::new((1.0, 2.0), (3.0, 4.0));
    round_trip(
        PgValue::Box(rect_to_hashable_f64s(rect)),
        PgValue::Box(rect_to_hashable_f64s(rect)),
        PgType::BOX,
        &rect,
    )?;
    round_trip(
        PgValue::BoxArray(vec![rect_to_hashable_f64s(rect)]),
        PgValue::BoxArray(vec![rect_to_hashable_f64s(rect)]),
        PgType::BOX_ARRAY,
        &vec![rect],
    )?;
    let mac = MacAddressEui48 {
        bytes: (0, 1, 2, 3, 4, 255),
    };
    round_trip(
        PgValue::Macaddr(mac),
        PgValue::Macaddr(mac),
        PgType::MACADDR,
        &mac,
    )?;
    round_trip(
        PgValue::MacaddrArray(vec![mac]),
        PgValue::MacaddrArray(vec![mac]),
        PgType::MACADDR_ARRAY,
        &vec![mac],
    )?;
    let mac = MacAddressEui64 {
        bytes: (0, 1, 2, 3, 4, 5, 6, 255),
    };
    round_trip(
        PgValue::Macaddr8(mac),
        PgValue::Macaddr8(mac),
        PgType::MACADDR8,
        &mac,
    )?;
    round_trip(
        PgValue::Macaddr8Array(vec![mac]),
        PgValue::Macaddr8Array(vec![mac]),
        PgType::MACADDR8_ARRAY,
        &vec![mac],
    )?;
    Ok(())
}

#[test]
fn malformed_and_unsupported_wire_values_return_errors() {
    for ty in [
        PgType::BOOL,
        PgType::BOOL_ARRAY,
        PgType::BYTEA_ARRAY,
        PgType::CHAR_ARRAY,
        PgType::BIT,
        PgType::BIT_ARRAY,
        PgType::VARBIT,
        PgType::VARBIT_ARRAY,
        PgType::FLOAT4,
        PgType::FLOAT4_ARRAY,
        PgType::FLOAT8,
        PgType::FLOAT8_ARRAY,
        PgType::INT2,
        PgType::INT2_ARRAY,
        PgType::INT2_VECTOR_ARRAY,
        PgType::INT4,
        PgType::INT4_ARRAY,
        PgType::INT8,
        PgType::INT8_ARRAY,
        PgType::NUMERIC,
        PgType::NUMERIC_ARRAY,
        PgType::MONEY,
        PgType::MONEY_ARRAY,
        PgType::JSON,
        PgType::JSON_ARRAY,
        PgType::JSONB,
        PgType::JSONB_ARRAY,
        PgType::VARCHAR_ARRAY,
        PgType::NAME_ARRAY,
        PgType::TEXT_ARRAY,
        PgType::XML_ARRAY,
        PgType::BOX,
        PgType::BOX_ARRAY,
        PgType::LSEG,
        PgType::LSEG_ARRAY,
        PgType::PATH_ARRAY,
        PgType::POINT,
        PgType::POINT_ARRAY,
        PgType::POLYGON_ARRAY,
        PgType::CIDR,
        PgType::CIDR_ARRAY,
        PgType::INET,
        PgType::INET_ARRAY,
        PgType::MACADDR,
        PgType::MACADDR_ARRAY,
        PgType::MACADDR8,
        PgType::MACADDR8_ARRAY,
        PgType::DATE,
        PgType::DATE_ARRAY,
        PgType::TIME,
        PgType::TIME_ARRAY,
        PgType::TIMESTAMP,
        PgType::TIMESTAMP_ARRAY,
        PgType::TIMESTAMPTZ,
        PgType::TIMESTAMPTZ_ARRAY,
        PgType::UUID,
        PgType::UUID_ARRAY,
        PgType::PG_LSN,
        PgType::PG_LSN_ARRAY,
        PgType::INT4_RANGE,
        PgType::INT8_RANGE,
        PgType::NUM_RANGE,
        PgType::DATE_RANGE,
        PgType::TS_RANGE,
        PgType::TSTZ_RANGE,
        PgType::INTERVAL,
        PgType::INTERVAL_ARRAY,
        PgType::TIMETZ,
        PgType::TIMETZ_ARRAY,
        PgType::VOID,
    ] {
        assert!(
            PgValue::from_sql(
                &ty,
                if matches!(ty, PgType::NUMERIC | PgType::NUMERIC_ARRAY | PgType::MONEY) {
                    &[255]
                } else {
                    &[]
                }
            )
            .is_err(),
            "{ty}"
        );
    }
    for ty in [PgType::TEXT, PgType::XML, PgType::NAME] {
        assert!(PgValue::from_sql(&ty, &[255]).is_err(), "{ty}");
    }
    assert!(matches!(
        PgValue::from_sql_null(&PgType::INT4),
        Ok(PgValue::Null)
    ));
}

#[test]
fn invalid_parameters_are_rejected_before_encoding() {
    for (value, ty) in [
        (PgValue::Bit((2, vec![0])), PgType::BIT),
        (PgValue::BitArray(vec![(2, vec![0])]), PgType::BIT_ARRAY),
        (PgValue::Varbit((Some(0), vec![0])), PgType::VARBIT),
        (
            PgValue::VarbitArray(vec![(Some(0), vec![0])]),
            PgType::VARBIT_ARRAY,
        ),
        (PgValue::Char((2, vec![0])), PgType::CHAR),
        (PgValue::CharArray(vec![(2, vec![0])]), PgType::CHAR_ARRAY),
        (PgValue::Varchar((Some(0), vec![0])), PgType::VARCHAR),
        (
            PgValue::VarcharArray(vec![(Some(0), vec![0])]),
            PgType::VARCHAR_ARRAY,
        ),
        (PgValue::Numeric("bad".into()), PgType::NUMERIC),
        (
            PgValue::NumericArray(vec!["bad".into()]),
            PgType::NUMERIC_ARRAY,
        ),
        (PgValue::Cidr("bad".into()), PgType::CIDR),
        (PgValue::CidrArray(vec!["bad".into()]), PgType::CIDR_ARRAY),
        (PgValue::Inet("bad".into()), PgType::INET),
        (PgValue::InetArray(vec!["bad".into()]), PgType::INET_ARRAY),
        (PgValue::Uuid("bad".into()), PgType::UUID),
        (PgValue::UuidArray(vec!["bad".into()]), PgType::UUID_ARRAY),
        (PgValue::Json("bad".into()), PgType::JSON),
        (PgValue::JsonArray(vec!["bad".into()]), PgType::JSON_ARRAY),
        (PgValue::Path(vec![]), PgType::PATH),
        (PgValue::Date(Date::Ymd((2024, 2, 30))), PgType::DATE),
        (
            PgValue::Time(Time {
                hour: 24,
                min: 0,
                sec: 0,
                micro: 0,
            }),
            PgType::TIME,
        ),
    ] {
        assert!(
            value.to_sql_checked(&ty, &mut BytesMut::new()).is_err(),
            "{value:?}"
        );
    }
    for offset in [
        Offset::EasternHemisphereSecs(86400),
        Offset::WesternHemisphereSecs(86400),
    ] {
        assert!(offset_to_fixed(offset).is_err());
    }
}

#[test]
fn bitfield_decoding_preserves_bit_lengths_and_hstore_preserves_nulls() -> TestResult {
    fn decode<T: ToSql>(native: &T, ty: PgType, expected: PgValue) -> TestResult {
        let mut wire = BytesMut::new();
        native.to_sql(&ty, &mut wire)?;
        assert_eq!(
            format!("{:?}", PgValue::from_sql(&ty, &wire)?),
            format!("{expected:?}")
        );
        Ok(())
    }
    let bits = BitVec::from_bytes(&[0b1010_0101]);
    decode(&bits, PgType::BIT, PgValue::Bit((8, vec![0b1010_0101])))?;
    decode(
        &bits,
        PgType::VARBIT,
        PgValue::BitVarying((Some(8), vec![0b1010_0101])),
    )?;
    decode(
        &vec![bits.clone()],
        PgType::BIT_ARRAY,
        PgValue::BitArray(vec![(8, vec![0b1010_0101])]),
    )?;
    decode(
        &vec![bits],
        PgType::VARBIT_ARRAY,
        PgValue::VarbitArray(vec![(Some(8), vec![0b1010_0101])]),
    )?;
    let ty = PgType::new("hstore".into(), 16384, Kind::Simple, "public".into());
    for value in [Some("value".to_string()), None] {
        let pairs = vec![("key".to_string(), value)];
        round_trip(
            PgValue::Hstore(pairs.clone()),
            PgValue::Hstore(pairs.clone()),
            ty.clone(),
            &HashMap::<String, Option<String>>::from_iter(pairs),
        )?;
    }
    assert!(matches!(
        PgValue::Null.to_sql_checked(&PgType::TEXT, &mut BytesMut::new())?,
        IsNull::Yes
    ));
    Ok(())
}

#[test]
fn geometry_and_mac_helpers_enforce_sizes_and_types() -> TestResult {
    let point = coord! { x: 1.5, y: -2.5 };
    let line = LineString::new(vec![point, point]);
    let expected = (1.5_f64.integer_decode(), (-2.5_f64).integer_decode());
    assert_eq!(
        linestring_to_hashable_f64s_tuple(line)?,
        (expected, expected)
    );
    for length in [0, 1, 3] {
        assert!(linestring_to_hashable_f64s_tuple(LineString::new(vec![point; length])).is_err());
    }
    let mac48 = MacAddressEui48 {
        bytes: (0, 1, 2, 3, 4, 5),
    };
    let mac64 = MacAddressEui64 {
        bytes: (0, 1, 2, 3, 4, 5, 6, 7),
    };
    assert!(mac48.to_sql(&PgType::TEXT, &mut BytesMut::new()).is_err());
    assert!(mac64.to_sql(&PgType::TEXT, &mut BytesMut::new()).is_err());
    assert!(mac_eui48_from_bytes(&[0; 5]).is_err());
    assert!(mac_eui64_from_bytes(&[0; 7]).is_err());
    assert!(!<MacAddressEui48 as ToSql>::accepts(&PgType::TEXT));
    assert!(!<MacAddressEui64 as FromSql>::accepts(&PgType::TEXT));
    Ok(())
}
