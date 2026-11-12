// `PgValue` <-> tokio-postgres conversions (the `FromSql`/`ToSql` impls and
// helpers), ported from wasmCloud's provider-sqldb-postgres reference impl.
//
// This file is `include!`d into a small `conversions` module by *both* the sync
// (`0.1.1-draft`) and async (`0.2.0`) bindings. bindgen emits a distinct-but-
// identical `PgValue` type per package version, and a `FromSql` impl lives on a
// concrete type, so each binding needs its own copy of the impls — but the logic
// is identical, so it lives here once. It refers to `PgValue` and the value
// sub-types (`Date`, `Time`, ...) by bare name; each including module brings its
// binding's copies into scope via `use` before the `include!`.
//
// Because it is pasted mid-module, this file must not carry a `//!` module doc
// or `use` the binding types itself.

use core::net::IpAddr;
use std::collections::HashMap;
use std::error::Error;
use std::str::FromStr;

use anyhow::{Context as _, bail};
use bigdecimal::num_traits::Float;
use bit_vec::BitVec;
use bytes::{BufMut, BytesMut};
use chrono::{
    DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Offset as _, Timelike,
    Utc,
};
use cidr::IpCidr;
use geo_types::{LineString, Point, Rect, coord};
use pg_bigdecimal::PgNumeric;
use postgres_types::{FromSql, IsNull, Kind, PgLsn, ToSql, Type as PgType, WrongType};
use tokio_postgres::Row;
use uuid::Uuid;

// ── Helper functions ────────────────────────────────────────────────────────

/// Build an `f64` from a mantissa, exponent and sign
fn f64_from_components(mantissa: u64, exponent: i16, sign: i8) -> f64 {
    let sign_f = sign as f64;
    let mantissa_f = mantissa as f64;
    let exponent_f = 2f64.powf(exponent as f64);
    sign_f * mantissa_f * exponent_f
}

/// Build an `f64` from a simple tuple of mantissa, exponent and sign
fn f64_from_tuple(t: &(u64, i16, i8)) -> f64 {
    f64_from_components(t.0, t.1, t.2)
}

/// Convert [`Rect`] into a tuple of points (two `HashableF64` pairs)
fn rect_to_hashable_f64s(r: Rect<f64>) -> ((HashableF64, HashableF64), (HashableF64, HashableF64)) {
    let (bottom_left_x, bottom_left_y) = r.min().x_y();
    let (top_right_x, top_right_y) = r.max().x_y();
    (
        (
            bottom_left_x.integer_decode(),
            bottom_left_y.integer_decode(),
        ),
        (top_right_x.integer_decode(), top_right_y.integer_decode()),
    )
}

/// Convert a [`LineString`] (expected to have exactly two points) into a tuple of `HashableF64` pairs
fn linestring_to_hashable_f64s_tuple(
    l: LineString<f64>,
) -> anyhow::Result<((HashableF64, HashableF64), (HashableF64, HashableF64))> {
    match linestring_to_hashable_f64s(l)[..] {
        [start, end] => Ok((start, end)),
        _ => bail!("unexpected number of points in line string"),
    }
}

/// Convert a [`LineString`] into a vector of point pairs
fn linestring_to_hashable_f64s(l: LineString<f64>) -> Vec<(HashableF64, HashableF64)> {
    l.into_points()
        .into_iter()
        .map(point_to_hashable_f64s)
        .collect::<Vec<_>>()
}

/// Convert a [`Point`] into two `HashableF64`s
fn point_to_hashable_f64s(p: Point<f64>) -> (HashableF64, HashableF64) {
    let (x, y) = p.x_y();
    (x.integer_decode(), y.integer_decode())
}

/// A guest's open `path`, which Postgres refuses without a point.
fn line_string(
    points: &[(HashableF64, HashableF64)],
) -> Result<LineString<f64>, Box<dyn Error + Sync + Send>> {
    if points.is_empty() {
        return Err("invalid path, no points specified".into());
    }
    Ok(points
        .iter()
        .map(|(x, y)| coord! { x: f64_from_tuple(x), y: f64_from_tuple(y) })
        .collect())
}

/// A guest's `polygon`, which Postgres refuses without a point.
fn polygon(
    points: &[(HashableF64, HashableF64)],
) -> Result<PgPolygon, Box<dyn Error + Sync + Send>> {
    if points.is_empty() {
        return Err("invalid polygon, no points specified".into());
    }
    Ok(PgPolygon(
        points
            .iter()
            .map(|(x, y)| Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y)))
            .collect(),
    ))
}

/// Most digits a guest's `numeric` may carry: the largest precision a declared
/// `numeric(p)` column holds. pg_bigdecimal's encoder is quadratic in digits,
/// about 0.1 ms here but 2 s at an unconstrained column's 131072. A larger
/// value can be bound as text and cast with `$1::text::numeric`.
const MAX_NUMERIC_DIGITS: i64 = 1000;

/// Longest `numeric` text parsed: the digits plus a sign, point and exponent.
const MAX_NUMERIC_TEXT_LEN: usize = 1024;

/// Parse a guest's `numeric` text for the encoder.
///
/// The encoder raises ten to the scale as a `u32`, so a negative scale (from
/// text like `1e9`) would wrap to a power the host cannot compute. Such a value
/// is rescaled to zero first.
fn parse_numeric(s: &str) -> Result<PgNumeric, Box<dyn Error + Sync + Send>> {
    if s.len() > MAX_NUMERIC_TEXT_LEN {
        return Err(format!("numeric text is longer than {MAX_NUMERIC_TEXT_LEN} bytes").into());
    }
    let value = pg_bigdecimal::BigDecimal::parse_bytes(s.as_bytes(), 10)
        .ok_or_else(|| format!("failed to parse numeric [{s}]"))?;
    let (_, scale) = value.as_bigint_and_exponent();
    let value = match scale {
        0.. => value,
        _ if scale < -MAX_NUMERIC_DIGITS => {
            return Err(format!("numeric [{s}] has more than {MAX_NUMERIC_DIGITS} digits").into());
        }
        _ => value.with_scale(0),
    };
    Ok(PgNumeric::new(Some(value)))
}

/// A `polygon`: its point count, then each point. geo-types' `Polygon` is not
/// encoded by postgres-types, and a `Vec<Point>` encodes as `point[]`.
#[derive(Debug)]
struct PgPolygon(Vec<Point<f64>>);

impl ToSql for PgPolygon {
    fn to_sql(
        &self,
        _: &PgType,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        out.put_i32(i32::try_from(self.0.len())?);
        for point in &self.0 {
            out.put_f64(point.x());
            out.put_f64(point.y());
        }
        Ok(IsNull::No)
    }

    fn accepts(ty: &PgType) -> bool {
        *ty == PgType::POLYGON
    }

    tokio_postgres::types::to_sql_checked!();
}

/// The points of a `polygon`, or of a `path` past its closed flag:
/// a point count, then each point's `x` and `y`.
///
/// The count is checked against the bytes that follow before any room is made,
/// so a value that overstates it is an error and not an allocation.
fn counted_points(
    ty: &PgType,
    raw: &[u8],
) -> Result<Vec<Point<f64>>, Box<dyn Error + Sync + Send>> {
    let (count, points) = raw
        .split_first_chunk::<4>()
        .ok_or_else(|| format!("invalid {ty}, no point count"))?;
    // A negative count matches no length.
    let count = usize::try_from(i32::from_be_bytes(*count)).ok();
    let (floats, rest) = points.as_chunks::<8>();
    if !rest.is_empty() || count.and_then(|count| count.checked_mul(2)) != Some(floats.len()) {
        return Err(format!("invalid {ty}, point count does not match its length").into());
    }
    Ok(floats
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[x, y]| Point::new(f64::from_be_bytes(*x), f64::from_be_bytes(*y)))
        .collect())
}

impl FromSql<'_> for PgPolygon {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        counted_points(ty, raw).map(PgPolygon)
    }

    fn accepts(ty: &PgType) -> bool {
        *ty == PgType::POLYGON
    }
}

/// A `path` column: whether it is closed, which `pg-value` has no place for,
/// then its points.
///
/// Not postgres-types' `LineString`, which makes room for as many points as
/// the value says it holds before reading any:
/// a negative count panics, and a large one aborts the host.
struct PgPath(Vec<Point<f64>>);

impl FromSql<'_> for PgPath {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let (_closed, points) = raw
            .split_first()
            .ok_or_else(|| format!("invalid {ty}, no closed flag"))?;
        counted_points(ty, points).map(PgPath)
    }

    fn accepts(ty: &PgType) -> bool {
        *ty == PgType::PATH
    }
}

/// The two end points of an `lseg`, as `pg-value` holds them.
type PointPair = ((HashableF64, HashableF64), (HashableF64, HashableF64));

/// An `lseg` parameter: its two end points' four `float8`s.
/// postgres-types has no encoder for one, and a `LineString` encodes as a
/// `path`, which Postgres refuses for an `lseg`.
#[derive(Debug)]
struct PgLseg<'a>(&'a PointPair);

impl ToSql for PgLseg<'_> {
    fn to_sql(
        &self,
        _: &PgType,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        let ((start_x, start_y), (end_x, end_y)) = self.0;
        for coordinate in [start_x, start_y, end_x, end_y] {
            out.put_f64(f64_from_tuple(coordinate));
        }
        Ok(IsNull::No)
    }

    fn accepts(ty: &PgType) -> bool {
        *ty == PgType::LSEG
    }

    tokio_postgres::types::to_sql_checked!();
}

// ── MAC address helpers ─────────────────────────────────────────────────────

fn mac_eui48_as_bytes(m: MacAddressEui48) -> [u8; 6] {
    [
        m.bytes.0, m.bytes.1, m.bytes.2, m.bytes.3, m.bytes.4, m.bytes.5,
    ]
}

fn mac_eui64_as_bytes(m: MacAddressEui64) -> [u8; 8] {
    [
        m.bytes.0, m.bytes.1, m.bytes.2, m.bytes.3, m.bytes.4, m.bytes.5, m.bytes.6, m.bytes.7,
    ]
}

// ── Date/Time conversions ───────────────────────────────────────────────────

fn date_to_naive(d: &Date) -> anyhow::Result<NaiveDate> {
    match d {
        Date::PositiveInfinity => Ok(NaiveDate::MAX),
        Date::NegativeInfinity => Ok(NaiveDate::MIN),
        Date::Ymd((year, month, day)) => NaiveDate::from_ymd_opt(*year, *month, *day)
            .with_context(|| format!("failed to build date from ymd ({year}/{month}/{day})")),
    }
}

fn naive_to_date(nd: NaiveDate) -> Date {
    match nd {
        NaiveDate::MAX => Date::PositiveInfinity,
        NaiveDate::MIN => Date::NegativeInfinity,
        nd => Date::Ymd((nd.year(), nd.month(), nd.day())),
    }
}

fn time_to_naive(t: &Time) -> anyhow::Result<NaiveTime> {
    NaiveTime::from_hms_micro_opt(t.hour, t.min, t.sec, t.micro).with_context(|| {
        format!(
            "failed to convert time [{}h {}m {}s {}micro]",
            t.hour, t.min, t.sec, t.micro
        )
    })
}

fn naive_to_time(nt: NaiveTime) -> Time {
    Time {
        hour: nt.hour(),
        min: nt.minute(),
        sec: nt.second(),
        // Intentional truncation: postgres has microsecond precision
        micro: nt.nanosecond() / 1_000,
    }
}

fn timestamp_to_naive(ts: &Timestamp) -> anyhow::Result<NaiveDateTime> {
    match &ts.date {
        Date::PositiveInfinity => Ok(NaiveDateTime::MAX),
        Date::NegativeInfinity => Ok(NaiveDateTime::MIN),
        Date::Ymd(_) => Ok(NaiveDateTime::new(
            date_to_naive(&ts.date)?,
            time_to_naive(&ts.time)?,
        )),
    }
}

fn naive_to_timestamp(ndt: NaiveDateTime) -> Timestamp {
    Timestamp {
        date: naive_to_date(ndt.date()),
        time: naive_to_time(ndt.time()),
    }
}

fn offset_to_fixed(offset: Offset) -> anyhow::Result<FixedOffset> {
    match offset {
        Offset::EasternHemisphereSecs(secs) => FixedOffset::east_opt(secs)
            .with_context(|| format!("failed to convert eastern hemisphere seconds [{secs}]")),
        Offset::WesternHemisphereSecs(secs) => FixedOffset::west_opt(secs)
            .with_context(|| format!("failed to convert western hemisphere seconds [{secs}]")),
    }
}

fn timestamptz_to_utc(tstz: &TimestampTz) -> anyhow::Result<DateTime<Utc>> {
    let fixed_offset = offset_to_fixed(tstz.offset)?;
    let timestamp = timestamp_to_naive(&tstz.timestamp)?;
    Ok(DateTime::<FixedOffset>::from_naive_utc_and_offset(timestamp, fixed_offset).into())
}

fn utc_to_timestamptz(dt: DateTime<Utc>) -> TimestampTz {
    TimestampTz {
        offset: Offset::WesternHemisphereSecs(dt.offset().fix().local_minus_utc()),
        timestamp: naive_to_timestamp(dt.naive_local()),
    }
}

// ── ToSql for MacAddressEui48 ───────────────────────────────────────────────

impl ToSql for MacAddressEui48 {
    fn to_sql(
        &self,
        ty: &PgType,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        match ty {
            &tokio_postgres::types::Type::MACADDR => {
                out.put_slice(&mac_eui48_as_bytes(*self));
                Ok(IsNull::No)
            }
            _ => Err("invalid Postgres type for EUI48 MAC address".into()),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        matches!(ty, &tokio_postgres::types::Type::MACADDR)
    }

    tokio_postgres::types::to_sql_checked!();
}

impl ToSql for MacAddressEui64 {
    fn to_sql(
        &self,
        ty: &PgType,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        match ty {
            &tokio_postgres::types::Type::MACADDR8 => {
                out.put_slice(&mac_eui64_as_bytes(*self));
                Ok(IsNull::No)
            }
            _ => Err("invalid Postgres type for EUI64 MAC address".into()),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        matches!(ty, &tokio_postgres::types::Type::MACADDR8)
    }

    tokio_postgres::types::to_sql_checked!();
}

// ── FromSql for MAC addresses ───────────────────────────────────────────────

fn mac_eui48_from_bytes(bytes: &[u8]) -> Result<MacAddressEui48, Box<dyn Error + Sync + Send>> {
    match bytes[..] {
        [o0, o1, o2, o3, o4, o5] => Ok(MacAddressEui48 {
            bytes: (o0, o1, o2, o3, o4, o5),
        }),
        _ => Err(format!(
            "unexpected number of bytes ({}) in EUI48 mac address",
            bytes.len()
        )
        .into()),
    }
}

fn mac_eui64_from_bytes(bytes: &[u8]) -> Result<MacAddressEui64, Box<dyn Error + Sync + Send>> {
    match bytes[..] {
        [o0, o1, o2, o3, o4, o5, o6, o7] => Ok(MacAddressEui64 {
            bytes: (o0, o1, o2, o3, o4, o5, o6, o7),
        }),
        _ => Err(format!(
            "unexpected number of bytes ({}) in EUI64 mac address",
            bytes.len()
        )
        .into()),
    }
}

impl FromSql<'_> for MacAddressEui48 {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match (ty, raw) {
            (&tokio_postgres::types::Type::MACADDR, bytes) if bytes.len() == 6 => {
                mac_eui48_from_bytes(bytes)
            }
            _ => Err("invalid type/raw input for EUI48 MAC address".into()),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        matches!(ty, &tokio_postgres::types::Type::MACADDR)
    }
}

impl FromSql<'_> for MacAddressEui64 {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match (ty, raw) {
            (&tokio_postgres::types::Type::MACADDR8, bytes) if bytes.len() == 8 => {
                mac_eui64_from_bytes(bytes)
            }
            _ => Err("invalid type/raw input for EUI64 MAC address".into()),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        matches!(ty, &tokio_postgres::types::Type::MACADDR8)
    }
}

/// Whether Postgres reads the binary form of `ty` as the value's text.
///
/// These are the types `&str` binds to (`text`, `varchar`, `bpchar`, `name`,
/// `unknown`, `citext` and the `ltree` types), plus `json`, `xml`, `refcursor`,
/// `"char"` and enums. A one-byte text is the binary form of a `"char"`. Any
/// other type, including fixed-width and extension types, would read the bytes
/// of the text as a different value.
fn reads_text(ty: &PgType) -> bool {
    <&str as ToSql>::accepts(ty)
        || matches!(
            *ty,
            PgType::JSON | PgType::XML | PgType::REFCURSOR | PgType::CHAR
        )
        || matches!(ty.kind(), Kind::Enum(_))
}

/// Whether Postgres reads `ty` from the raw bytes of a text.
///
/// The `ltree` types are left out: their binary form starts with a version
/// byte, which `&str` writes for them but the raw bytes of a `varchar` or
/// `char` value do not have.
fn reads_raw_text(ty: &PgType) -> bool {
    reads_text(ty) && !matches!(ty.name(), "ltree" | "lquery" | "ltxtquery")
}

/// The type a domain is defined over, or `ty` itself.
fn domain_base(mut ty: &PgType) -> &PgType {
    while let Kind::Domain(inner) = ty.kind() {
        ty = inner;
    }
    ty
}

/// `oid` and the `reg*` aliases share `int4`'s four-byte encoding, so an `int4`
/// value can stand in for them (`WHERE oid = $1`).
fn is_oid_like(ty: &PgType) -> bool {
    [
        PgType::OID,
        PgType::REGCLASS,
        PgType::REGCOLLATION,
        PgType::REGCONFIG,
        PgType::REGDICTIONARY,
        PgType::REGNAMESPACE,
        PgType::REGOPER,
        PgType::REGOPERATOR,
        PgType::REGPROC,
        PgType::REGPROCEDURE,
        PgType::REGROLE,
        PgType::REGTYPE,
    ]
    .contains(ty)
}

/// Refuse a value bound to a parameter of a type it could be misread as.
///
/// The inner encoders do not look at the parameter type, so without this check
/// Postgres would read, for example, the eight bytes of an `int8` as a `float8`
/// (or fail with an error that does not name the parameter's type). A domain
/// is checked as its base type, and an array by its element type (after
/// `check_array_type` has checked its depth). Values not listed here are left
/// to their own encoders.
fn check_scalar_type(value: &PgValue, ty: &PgType) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut base = domain_base(ty);
    for _ in 0..array_depth(value) {
        if let Kind::Array(member) = base.kind() {
            base = domain_base(member);
        }
    }
    let wrong =
        |fits: bool, rust: fn(PgType) -> WrongType| -> Result<(), Box<dyn Error + Sync + Send>> {
            if fits {
                Ok(())
            } else {
                Err(Box::new(rust(ty.clone())))
            }
        };
    match value {
        PgValue::BigInt(_)
        | PgValue::Int8(_)
        | PgValue::BigSerial(_)
        | PgValue::Serial8(_)
        | PgValue::Int8Array(_) => wrong(<i64 as ToSql>::accepts(base), WrongType::new::<i64>),
        PgValue::Integer(_)
        | PgValue::Int(_)
        | PgValue::Int4(_)
        | PgValue::Serial(_)
        | PgValue::Serial4(_)
        | PgValue::Int4Array(_) => wrong(
            <i32 as ToSql>::accepts(base) || is_oid_like(base),
            WrongType::new::<i32>,
        ),
        PgValue::SmallInt(_)
        | PgValue::Int2(_)
        | PgValue::SmallSerial(_)
        | PgValue::Serial2(_)
        | PgValue::Int2Array(_)
        | PgValue::Int2Vector(_)
        | PgValue::Int2VectorArray(_) => {
            wrong(<i16 as ToSql>::accepts(base), WrongType::new::<i16>)
        }
        PgValue::Double(_) | PgValue::Float8(_) | PgValue::Float8Array(_) => {
            wrong(<f64 as ToSql>::accepts(base), WrongType::new::<f64>)
        }
        PgValue::Real(_) | PgValue::Float4(_) | PgValue::Float4Array(_) => {
            wrong(<f32 as ToSql>::accepts(base), WrongType::new::<f32>)
        }
        PgValue::Bool(_) | PgValue::Boolean(_) | PgValue::BoolArray(_) => {
            wrong(<bool as ToSql>::accepts(base), WrongType::new::<bool>)
        }
        PgValue::Date(_) | PgValue::DateArray(_) => {
            wrong(*base == PgType::DATE, WrongType::new::<NaiveDate>)
        }
        PgValue::Time(_) | PgValue::TimeArray(_) => {
            wrong(*base == PgType::TIME, WrongType::new::<NaiveTime>)
        }
        // `timestamp` and `timestamptz` share one binary encoding (microseconds
        // since 2000-01-01), so a value of either binds to either.
        PgValue::Timestamp(_) | PgValue::TimestampArray(_) => wrong(
            matches!(*base, PgType::TIMESTAMP | PgType::TIMESTAMPTZ),
            WrongType::new::<NaiveDateTime>,
        ),
        PgValue::TimestampTz(_) | PgValue::TimestampTzArray(_) => wrong(
            matches!(*base, PgType::TIMESTAMP | PgType::TIMESTAMPTZ),
            WrongType::new::<DateTime<Utc>>,
        ),
        // `tsquery` and `pg_snapshot` values are encoded as their text.
        PgValue::Text(_)
        | PgValue::TextArray(_)
        | PgValue::Name(_)
        | PgValue::NameArray(_)
        | PgValue::Xml(_)
        | PgValue::XmlArray(_)
        | PgValue::TsQuery(_)
        | PgValue::PgSnapshot(_) => wrong(reads_text(base), WrongType::new::<String>),
        PgValue::Varchar(_)
        | PgValue::VarcharArray(_)
        | PgValue::Char(_)
        | PgValue::CharArray(_) => wrong(reads_raw_text(base), WrongType::new::<String>),
        PgValue::Bytea(_) | PgValue::ByteaArray(_) => {
            wrong(*base == PgType::BYTEA, WrongType::new::<Vec<u8>>)
        }
        PgValue::Json(_) | PgValue::Jsonb(_) | PgValue::JsonArray(_) | PgValue::JsonbArray(_) => {
            wrong(
                matches!(*base, PgType::JSON | PgType::JSONB),
                WrongType::new::<serde_json::Value>,
            )
        }
        PgValue::Bit(_)
        | PgValue::BitVarying(_)
        | PgValue::Varbit(_)
        | PgValue::BitArray(_)
        | PgValue::VarbitArray(_) => wrong(
            matches!(*base, PgType::BIT | PgType::VARBIT),
            WrongType::new::<BitVec>,
        ),
        // Sixteen bytes each, as are an `interval` and one another.
        PgValue::Uuid(_) | PgValue::UuidArray(_) => {
            wrong(*base == PgType::UUID, WrongType::new::<Uuid>)
        }
        PgValue::Point(_) | PgValue::PointArray(_) => {
            wrong(*base == PgType::POINT, WrongType::new::<Point<f64>>)
        }
        // Thirty-two bytes each.
        PgValue::Box(_) | PgValue::BoxArray(_) => {
            wrong(*base == PgType::BOX, WrongType::new::<Rect<f64>>)
        }
        PgValue::Lseg(_) | PgValue::LsegArray(_) => {
            wrong(*base == PgType::LSEG, WrongType::new::<PgLseg>)
        }
        // Eight bytes, as are an `int8`, a `float8` and a `timestamp`.
        PgValue::PgLsn(_) | PgValue::PgLsnArray(_) => {
            wrong(*base == PgType::PG_LSN, WrongType::new::<PgLsn>)
        }
        PgValue::Path(_) | PgValue::PathArray(_) => {
            wrong(*base == PgType::PATH, WrongType::new::<LineString<f64>>)
        }
        PgValue::Polygon(_) | PgValue::PolygonArray(_) => {
            wrong(*base == PgType::POLYGON, WrongType::new::<PgPolygon>)
        }
        _ => Ok(()),
    }
}

/// How many array levels the encoder for `value` expects its parameter type to
/// have.
fn array_depth(value: &PgValue) -> usize {
    match value {
        PgValue::Int2VectorArray(_) => 2,
        PgValue::Int8Array(_)
        | PgValue::BoolArray(_)
        | PgValue::Float8Array(_)
        | PgValue::Float4Array(_)
        | PgValue::Int4Array(_)
        | PgValue::NumericArray(_)
        | PgValue::Int2Array(_)
        | PgValue::Int2Vector(_)
        | PgValue::BitArray(_)
        | PgValue::VarbitArray(_)
        | PgValue::ByteaArray(_)
        | PgValue::CharArray(_)
        | PgValue::VarcharArray(_)
        | PgValue::CidrArray(_)
        | PgValue::InetArray(_)
        | PgValue::MacaddrArray(_)
        | PgValue::Macaddr8Array(_)
        | PgValue::BoxArray(_)
        | PgValue::CircleArray(_)
        | PgValue::LineArray(_)
        | PgValue::LsegArray(_)
        | PgValue::PathArray(_)
        | PgValue::PointArray(_)
        | PgValue::PolygonArray(_)
        | PgValue::DateArray(_)
        | PgValue::IntervalArray(_)
        | PgValue::TimeArray(_)
        | PgValue::TimeTzArray(_)
        | PgValue::TimestampArray(_)
        | PgValue::TimestampTzArray(_)
        | PgValue::JsonArray(_)
        | PgValue::JsonbArray(_)
        | PgValue::MoneyArray(_)
        | PgValue::PgLsnArray(_)
        | PgValue::NameArray(_)
        | PgValue::TextArray(_)
        | PgValue::XmlArray(_)
        | PgValue::UuidArray(_) => 1,
        PgValue::Null
        | PgValue::BigInt(_)
        | PgValue::Int8(_)
        | PgValue::BigSerial(_)
        | PgValue::Serial8(_)
        | PgValue::Bool(_)
        | PgValue::Boolean(_)
        | PgValue::Double(_)
        | PgValue::Float8(_)
        | PgValue::Real(_)
        | PgValue::Float4(_)
        | PgValue::Integer(_)
        | PgValue::Int(_)
        | PgValue::Int4(_)
        | PgValue::Numeric(_)
        | PgValue::Decimal(_)
        | PgValue::Serial(_)
        | PgValue::Serial4(_)
        | PgValue::SmallInt(_)
        | PgValue::Int2(_)
        | PgValue::SmallSerial(_)
        | PgValue::Serial2(_)
        | PgValue::Bit(_)
        | PgValue::BitVarying(_)
        | PgValue::Varbit(_)
        | PgValue::Bytea(_)
        | PgValue::Char(_)
        | PgValue::Varchar(_)
        | PgValue::Cidr(_)
        | PgValue::Inet(_)
        | PgValue::Macaddr(_)
        | PgValue::Macaddr8(_)
        | PgValue::Box(_)
        | PgValue::Circle(_)
        | PgValue::Line(_)
        | PgValue::Lseg(_)
        | PgValue::Path(_)
        | PgValue::Point(_)
        | PgValue::Polygon(_)
        | PgValue::Date(_)
        | PgValue::Interval(_)
        | PgValue::Time(_)
        | PgValue::TimeTz(_)
        | PgValue::Timestamp(_)
        | PgValue::TimestampTz(_)
        | PgValue::Json(_)
        | PgValue::Jsonb(_)
        | PgValue::Money(_)
        | PgValue::PgLsn(_)
        | PgValue::PgSnapshot(_)
        | PgValue::TxidSnapshot(_)
        | PgValue::Name(_)
        | PgValue::Text(_)
        | PgValue::Xml(_)
        | PgValue::TsQuery(_)
        | PgValue::TsVector(_)
        | PgValue::Uuid(_)
        | PgValue::Hstore(_) => 0,
    }
}

/// Refuse an array value bound to a parameter that is not an array as deep.
///
/// postgres-types' slice encoder panics on any other parameter type, and the
/// value and the SQL both come from the guest.
fn check_array_type(value: &PgValue, ty: &PgType) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut level = ty;
    for _ in 0..array_depth(value) {
        match level.kind() {
            Kind::Array(member) => level = member,
            _ => return Err(Box::new(WrongType::new::<Vec<PgValue>>(ty.clone()))),
        }
    }
    Ok(())
}

// ── ToSql for PgValue ───────────────────────────────────────────────────────

impl ToSql for PgValue {
    fn to_sql(
        &self,
        ty: &PgType,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        check_array_type(self, domain_base(ty))?;
        check_scalar_type(self, ty)?;
        // Some encoders pick their format by type (the `jsonb` and `ltree`
        // version bytes), so they get the type a domain is defined over.
        let ty = domain_base(ty);
        match self {
            PgValue::Null => Ok(IsNull::Yes),
            // Numeric
            PgValue::BigInt(n) | PgValue::Int8(n) => n.to_sql(ty, out),
            PgValue::Int8Array(ns) => ns.to_sql(ty, out),
            PgValue::BigSerial(n) | PgValue::Serial8(n) => n.to_sql(ty, out),
            PgValue::Bool(n) | PgValue::Boolean(n) => n.to_sql(ty, out),
            PgValue::BoolArray(ns) => ns.to_sql(ty, out),
            PgValue::Double(d) | PgValue::Float8(d) => f64_from_tuple(d).to_sql(ty, out),
            PgValue::Float8Array(ds) => ds
                .iter()
                .map(f64_from_tuple)
                .collect::<Vec<_>>()
                .to_sql(ty, out),
            PgValue::Real(d) | PgValue::Float4(d) => (f64_from_tuple(d) as f32).to_sql(ty, out),
            PgValue::Float4Array(ds) => ds
                .iter()
                .map(|d| f64_from_tuple(d) as f32)
                .collect::<Vec<_>>()
                .to_sql(ty, out),
            PgValue::Integer(n) | PgValue::Int(n) | PgValue::Int4(n) => n.to_sql(ty, out),
            PgValue::Int4Array(ns) => ns.to_sql(ty, out),
            PgValue::Numeric(s) | PgValue::Decimal(s) | PgValue::Money(s) => {
                parse_numeric(s)?.to_sql(ty, out)
            }
            PgValue::NumericArray(ss) | PgValue::MoneyArray(ss) => ss
                .iter()
                .map(|s| parse_numeric(s))
                .collect::<Result<Vec<PgNumeric>, _>>()?
                .to_sql(ty, out),
            PgValue::SmallInt(n) | PgValue::Int2(n) => n.to_sql(ty, out),
            PgValue::Int2Array(ns) => ns.to_sql(ty, out),
            PgValue::Int2Vector(ns) => ns.to_sql(ty, out),
            PgValue::Int2VectorArray(ns) => ns.to_sql(ty, out),
            PgValue::Serial(n) | PgValue::Serial4(n) => n.to_sql(ty, out),
            PgValue::SmallSerial(n) | PgValue::Serial2(n) => n.to_sql(ty, out),

            // Bytes
            PgValue::Bit((exact_size, bytes)) => {
                if bytes.len() != *exact_size as usize {
                    return Err("bitfield size does not match".into());
                }
                bytes.as_slice().to_sql(ty, out)
            }
            PgValue::BitArray(many_bits) => {
                let mut vec: Vec<Vec<u8>> = Vec::new();
                for (exact_size, bytes) in many_bits.iter() {
                    if bytes.len() != *exact_size as usize {
                        return Err("bitfield size does not match".into());
                    }
                    vec.push(bytes.to_vec());
                }
                vec.to_sql(ty, out)
            }
            PgValue::BitVarying((limit, bytes)) | PgValue::Varbit((limit, bytes)) => {
                if limit.is_some_and(|limit| bytes.len() > limit as usize) {
                    return Err("bit field length is greater than limit".into());
                }
                bytes.as_slice().to_sql(ty, out)
            }
            PgValue::VarbitArray(many_varbits) => {
                let mut valid_varbits: Vec<Vec<u8>> = Vec::new();
                for (limit, bytes) in many_varbits {
                    if limit.is_some_and(|limit| bytes.len() > limit as usize) {
                        return Err("bit field length is greater than limit".into());
                    }
                    valid_varbits.push(bytes.to_vec());
                }
                valid_varbits.to_sql(ty, out)
            }
            PgValue::Bytea(bytes) => bytes.as_slice().to_sql(ty, out),
            PgValue::ByteaArray(many_bytes) => many_bytes
                .iter()
                .map(|b| b.as_slice())
                .collect::<Vec<&[u8]>>()
                .to_sql(ty, out),

            // Characters
            PgValue::Char((len, bytes)) => {
                if bytes.len() != *len as usize {
                    return Err("char length does not match specified size".into());
                }
                bytes.as_slice().to_sql(ty, out)
            }
            PgValue::CharArray(many_chars) => {
                let mut valid_chars: Vec<&[u8]> = Vec::new();
                for (len, bytes) in many_chars {
                    if bytes.len() != *len as usize {
                        return Err("char length does not match specified size".into());
                    }
                    valid_chars.push(bytes.as_slice());
                }
                valid_chars.to_sql(ty, out)
            }
            PgValue::Varchar((maybe_len, bytes)) => {
                if let Some(limit) = maybe_len
                    && bytes.len() > *limit as usize
                {
                    return Err(format!(
                        "char length [{}] does not match specified limit [{limit}]",
                        bytes.len(),
                    )
                    .into());
                }
                bytes.as_slice().to_sql(ty, out)
            }
            PgValue::VarcharArray(vs) => {
                let mut valid_varchars: Vec<&[u8]> = Vec::new();
                for (maybe_len, bytes) in vs {
                    if let Some(limit) = maybe_len
                        && bytes.len() > *limit as usize
                    {
                        return Err(format!(
                            "char length [{}] does not match specified limit [{limit}]",
                            bytes.len(),
                        )
                        .into());
                    }
                    valid_varchars.push(bytes.as_slice());
                }
                valid_varchars.to_sql(ty, out)
            }

            // Networking
            PgValue::Cidr(cidr) => IpCidr::from_str(cidr)
                .map_err(|e| format!("invalid cidr: {e}"))?
                .to_sql(ty, out),
            PgValue::CidrArray(cidrs) => cidrs
                .iter()
                .map(|v| IpCidr::from_str(v).map_err(|e| format!("invalid cidr: {e}")))
                .collect::<Result<Vec<IpCidr>, _>>()?
                .to_sql(ty, out),
            PgValue::Inet(addr) => IpAddr::from_str(addr)
                .map_err(|e| format!("invalid address: {e}"))?
                .to_sql(ty, out),
            PgValue::InetArray(inets) => inets
                .iter()
                .map(|v| IpAddr::from_str(v).map_err(|e| format!("invalid address: {e}")))
                .collect::<Result<Vec<IpAddr>, _>>()?
                .to_sql(ty, out),

            // EUI-48 (octets)
            PgValue::Macaddr(m) => m.to_sql(ty, out),
            PgValue::MacaddrArray(macs) => macs.clone().to_sql(ty, out),

            // EUI-64 (deprecated)
            PgValue::Macaddr8(m) => m.to_sql(ty, out),
            PgValue::Macaddr8Array(macs) => macs.clone().to_sql(ty, out),

            // Geo
            PgValue::Circle(_) | PgValue::CircleArray(_) => {
                Err("circle & circle[] are not supported".into())
            }
            PgValue::Box(((start_x, start_y), (end_x, end_y))) => {
                let start_x = f64_from_tuple(start_x);
                let start_y = f64_from_tuple(start_y);
                let end_x = f64_from_tuple(end_x);
                let end_y = f64_from_tuple(end_y);
                Rect::<f64>::new(
                    coord! { x: start_x, y: start_y },
                    coord! { x: end_x, y: end_y },
                )
                .to_sql(ty, out)
            }
            // Postgres holds a line as the coefficients of `Ax + By + C = 0`,
            // not as the two points `pg-value` has for one.
            PgValue::Line(_) | PgValue::LineArray(_) => {
                Err("line & line[] are not supported (consider a cast like $1::text::line)".into())
            }
            PgValue::Lseg(lseg) => PgLseg(lseg).to_sql(ty, out),
            PgValue::LsegArray(lsegs) => {
                lsegs.iter().map(PgLseg).collect::<Vec<_>>().to_sql(ty, out)
            }
            PgValue::BoxArray(boxes) => boxes
                .iter()
                .map(|((start_x, start_y), (end_x, end_y))| {
                    Rect::<f64>::new(
                        coord! { x: f64_from_tuple(start_x), y: f64_from_tuple(start_y) },
                        coord! { x: f64_from_tuple(end_x), y: f64_from_tuple(end_y) },
                    )
                })
                .collect::<Vec<Rect<f64>>>()
                .to_sql(ty, out),
            PgValue::Point((x, y)) => {
                Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y)).to_sql(ty, out)
            }
            PgValue::PointArray(points) => points
                .iter()
                .map(|(x, y)| Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y)))
                .collect::<Vec<Point>>()
                .to_sql(ty, out),
            PgValue::Path(points) => line_string(points)?.to_sql(ty, out),
            PgValue::Polygon(points) => polygon(points)?.to_sql(ty, out),
            PgValue::PathArray(paths) => paths
                .iter()
                .map(|points| line_string(points))
                .collect::<Result<Vec<_>, _>>()?
                .to_sql(ty, out),
            PgValue::PolygonArray(polygons) => polygons
                .iter()
                .map(|points| polygon(points))
                .collect::<Result<Vec<_>, _>>()?
                .to_sql(ty, out),

            // Date-time
            PgValue::Date(d) => {
                let d: NaiveDate = date_to_naive(d)?;
                d.to_sql(ty, out)
            }
            PgValue::DateArray(ds) => ds
                .iter()
                .map(date_to_naive)
                .collect::<Result<Vec<NaiveDate>, _>>()?
                .to_sql(ty, out),
            PgValue::Interval(_) | PgValue::IntervalArray(_) => Err(
                "interval not supported (consider using a cast like 'value'::text::interval)"
                    .into(),
            ),
            PgValue::Time(t) => {
                let t = time_to_naive(t)?;
                t.to_sql(ty, out)
            }
            PgValue::TimeArray(ts) => ts
                .iter()
                .map(time_to_naive)
                .collect::<Result<Vec<NaiveTime>, _>>()?
                .to_sql(ty, out),
            PgValue::TimeTz(_) | PgValue::TimeTzArray(_) => Err(
                "timetz not supported (consider using a cast like 'value'::text::timetz)".into(),
            ),
            PgValue::Timestamp(ts) => {
                let ts = timestamp_to_naive(ts)?;
                ts.to_sql(ty, out)
            }
            PgValue::TimestampArray(tss) => tss
                .iter()
                .map(timestamp_to_naive)
                .collect::<Result<Vec<NaiveDateTime>, _>>()?
                .to_sql(ty, out),
            PgValue::TimestampTz(tstz) => {
                let tstz = timestamptz_to_utc(tstz)?;
                tstz.to_sql(ty, out)
            }
            PgValue::TimestampTzArray(tstzs) => tstzs
                .iter()
                .map(timestamptz_to_utc)
                .collect::<Result<Vec<DateTime<Utc>>, _>>()?
                .to_sql(ty, out),

            // JSON
            PgValue::Json(s) | PgValue::Jsonb(s) => serde_json::Value::from_str(s)
                .map_err(|e| format!("failed to parse JSON: {e}"))?
                .to_sql(ty, out),
            PgValue::JsonArray(json_strings) | PgValue::JsonbArray(json_strings) => json_strings
                .iter()
                .map(|s| serde_json::Value::from_str(s))
                .collect::<Result<Vec<serde_json::Value>, _>>()?
                .to_sql(ty, out),

            // Postgres-internal
            PgValue::PgLsn(offset) => PgLsn::from(*offset).to_sql(ty, out),
            PgValue::PgLsnArray(offsets) => offsets
                .iter()
                .cloned()
                .map(PgLsn::from)
                .collect::<Vec<PgLsn>>()
                .to_sql(ty, out),
            PgValue::PgSnapshot((xmin, xmax, xip_list)) => format!(
                "{xmin}:{xmax}:{}",
                xip_list
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            )
            .to_sql(ty, out),
            PgValue::TxidSnapshot(n) => n.to_sql(ty, out),

            // Text
            PgValue::Name(s) => s.to_sql(ty, out),
            PgValue::NameArray(ss) => ss.to_sql(ty, out),
            PgValue::Text(s) => s.to_sql(ty, out),
            PgValue::TextArray(ss) => ss.to_sql(ty, out),
            PgValue::Xml(s) => s.to_sql(ty, out),
            PgValue::XmlArray(ss) => ss.to_sql(ty, out),

            // Full Text Search
            PgValue::TsQuery(s) => s.to_sql(ty, out),
            PgValue::TsVector(_) => Err(
                "tsvector not supported (consider using a cast like 'value'::text::tsvector)"
                    .into(),
            ),

            // UUIDs
            PgValue::Uuid(s) => Uuid::from_str(s)?.to_sql(ty, out),
            PgValue::UuidArray(ss) => ss
                .iter()
                .map(|v| Uuid::from_str(v.as_ref()))
                .collect::<Result<Vec<Uuid>, _>>()?
                .to_sql(ty, out),

            // Hstore
            PgValue::Hstore(h) => {
                let map = HashMap::<String, Option<String>>::from_iter(h.iter().cloned());
                map.to_sql(ty, out)
            }
        }
    }

    fn accepts(_ty: &PgType) -> bool {
        // Accept all types; `to_sql` refuses a scalar whose type does not match
        // the parameter (see `check_scalar_type`) and gives specific errors
        true
    }

    tokio_postgres::types::to_sql_checked!();
}

// ── FromSql for PgValue ─────────────────────────────────────────────────────

impl FromSql<'_> for PgValue {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match ty {
            &tokio_postgres::types::Type::BOOL => Ok(PgValue::Bool(bool::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::BOOL_ARRAY => {
                Ok(PgValue::BoolArray(Vec::<bool>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::BYTEA => {
                let buf = Vec::<u8>::from_sql(ty, raw)?;
                Ok(PgValue::Bytea(buf))
            }
            &tokio_postgres::types::Type::BYTEA_ARRAY => {
                let buf = Vec::<Vec<u8>>::from_sql(ty, raw)?;
                Ok(PgValue::ByteaArray(buf))
            }
            &tokio_postgres::types::Type::CHAR => {
                let s = Vec::<u8>::from_sql(ty, raw)?;
                let len = s.len().try_into()?;
                Ok(PgValue::Char((len, s)))
            }
            &tokio_postgres::types::Type::CHAR_ARRAY => {
                let list = Vec::<Vec<u8>>::from_sql(ty, raw)?;
                let mut cs = Vec::new();
                for c in list {
                    cs.push((c.len().try_into()?, c));
                }
                Ok(PgValue::CharArray(cs))
            }
            &tokio_postgres::types::Type::BIT => {
                let vec = BitVec::from_sql(ty, raw)?;
                let len = vec.len().try_into()?;
                Ok(PgValue::Bit((len, vec.to_bytes())))
            }
            &tokio_postgres::types::Type::BIT_ARRAY => {
                let vecs = Vec::<BitVec>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.len().try_into().map(|len| (len, v.to_bytes())))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(PgValue::BitArray(vecs))
            }
            &tokio_postgres::types::Type::VARBIT => {
                let vec = BitVec::from_sql(ty, raw)?;
                let len = vec.len().try_into()?;
                Ok(PgValue::BitVarying((Some(len), vec.to_bytes())))
            }
            &tokio_postgres::types::Type::VARBIT_ARRAY => {
                let varbits = Vec::<BitVec>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| {
                        let len = v.len().try_into().ok();
                        (len, v.to_bytes())
                    })
                    .collect::<Vec<_>>();
                Ok(PgValue::VarbitArray(varbits))
            }
            &tokio_postgres::types::Type::FLOAT4 => {
                Ok(PgValue::Float4(f32::from_sql(ty, raw)?.integer_decode()))
            }
            &tokio_postgres::types::Type::FLOAT4_ARRAY => Ok(PgValue::Float4Array(
                Vec::<f32>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|f| f.integer_decode())
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::FLOAT8 => {
                Ok(PgValue::Float8(f64::from_sql(ty, raw)?.integer_decode()))
            }
            &tokio_postgres::types::Type::FLOAT8_ARRAY => Ok(PgValue::Float8Array(
                Vec::<f64>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|f| f.integer_decode())
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::INT2 => Ok(PgValue::Int2(i16::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::INT2_ARRAY => {
                Ok(PgValue::Int2Array(Vec::<i16>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::INT2_VECTOR => {
                Ok(PgValue::Int2Vector(Vec::<i16>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::INT2_VECTOR_ARRAY => Ok(PgValue::Int2VectorArray(
                Vec::<Vec<i16>>::from_sql(ty, raw)?,
            )),
            &tokio_postgres::types::Type::INT4 => Ok(PgValue::Int4(i32::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::INT4_RANGE
            | &tokio_postgres::types::Type::INT4_RANGE_ARRAY
            | &tokio_postgres::types::Type::INT4MULTI_RANGE
            | &tokio_postgres::types::Type::INT4MULTI_RANGE_ARRAY => {
                Err("int4 ranges are not yet supported".into())
            }
            &tokio_postgres::types::Type::INT4_ARRAY => Ok(PgValue::Int4Array(
                Vec::<i32>::from_sql(ty, raw)?.into_iter().collect(),
            )),
            &tokio_postgres::types::Type::INT8 => Ok(PgValue::Int8(i64::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::INT8_ARRAY => {
                Ok(PgValue::Int8Array(Vec::<i64>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::INT8MULTI_RANGE
            | &tokio_postgres::types::Type::INT8MULTI_RANGE_ARRAY
            | &tokio_postgres::types::Type::INT8_RANGE
            | &tokio_postgres::types::Type::INT8_RANGE_ARRAY => {
                Err("int8 ranges are not yet supported".into())
            }

            &tokio_postgres::types::Type::MONEY => Ok(PgValue::Money(Numeric::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::MONEY_ARRAY => {
                Ok(PgValue::MoneyArray(Vec::<Numeric>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::NUMERIC => {
                Ok(PgValue::Numeric(Numeric::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::NUMERIC_ARRAY => {
                Ok(PgValue::NumericArray(Vec::<Numeric>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::NUMMULTI_RANGE
            | &tokio_postgres::types::Type::NUMMULTI_RANGE_ARRAY
            | &tokio_postgres::types::Type::NUM_RANGE
            | &tokio_postgres::types::Type::NUM_RANGE_ARRAY => {
                Err("numeric ranges are not yet supported".into())
            }

            // JSON
            &tokio_postgres::types::Type::JSON => Ok(PgValue::Json(
                serde_json::Value::from_sql(ty, raw)?.to_string(),
            )),
            &tokio_postgres::types::Type::JSON_ARRAY => Ok(PgValue::JsonArray(
                Vec::<serde_json::Value>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::JSONB => Ok(PgValue::Jsonb(
                serde_json::Value::from_sql(ty, raw)?.to_string(),
            )),
            &tokio_postgres::types::Type::JSONB_ARRAY => Ok(PgValue::JsonbArray(
                Vec::<serde_json::Value>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::VARCHAR => Ok(PgValue::Varchar(
                Vec::<u8>::from_sql(ty, raw).map(|v| (None, v))?,
            )),
            &tokio_postgres::types::Type::VARCHAR_ARRAY => Ok(PgValue::VarcharArray(
                Vec::<Vec<u8>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|s| (None, s))
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::NAME => Ok(PgValue::Name(String::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::NAME_ARRAY => {
                Ok(PgValue::NameArray(Vec::<String>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::TEXT => Ok(PgValue::Text(String::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::TEXT_ARRAY => {
                Ok(PgValue::TextArray(Vec::<String>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::XML => Ok(PgValue::Xml(String::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::XML_ARRAY => {
                Ok(PgValue::XmlArray(Vec::<String>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::BOX => Ok(PgValue::Box(rect_to_hashable_f64s(
                Rect::<f64>::from_sql(ty, raw)?,
            ))),
            &tokio_postgres::types::Type::BOX_ARRAY => Ok(PgValue::BoxArray(
                Vec::<Rect<f64>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(rect_to_hashable_f64s)
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::CIRCLE | &tokio_postgres::types::Type::CIRCLE_ARRAY => {
                Err("circle & circle[] are not supported".into())
            }
            &tokio_postgres::types::Type::LINE => Ok(PgValue::Line(
                linestring_to_hashable_f64s_tuple(LineString::<f64>::from_sql(ty, raw)?)?,
            )),
            &tokio_postgres::types::Type::LINE_ARRAY => Ok(PgValue::LineArray(
                Vec::<LineString<f64>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(linestring_to_hashable_f64s_tuple)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            &tokio_postgres::types::Type::LSEG => Ok(PgValue::Lseg(
                linestring_to_hashable_f64s_tuple(LineString::<f64>::from_sql(ty, raw)?)?,
            )),
            &tokio_postgres::types::Type::LSEG_ARRAY => Ok(PgValue::LsegArray(
                Vec::<LineString<f64>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(linestring_to_hashable_f64s_tuple)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            &tokio_postgres::types::Type::PATH => Ok(PgValue::Path(
                PgPath::from_sql(ty, raw)?
                    .0
                    .into_iter()
                    .map(point_to_hashable_f64s)
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::PATH_ARRAY => Ok(PgValue::PathArray(
                Vec::<PgPath>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.0.into_iter().map(point_to_hashable_f64s).collect())
                    .collect::<Vec<Vec<_>>>(),
            )),
            &tokio_postgres::types::Type::POINT => {
                let point = Point::<f64>::from_sql(ty, raw)?;
                Ok(PgValue::Point(point_to_hashable_f64s(point)))
            }
            &tokio_postgres::types::Type::POINT_ARRAY => Ok(PgValue::PointArray(
                Vec::<Point<f64>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(point_to_hashable_f64s)
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::POLYGON => Ok(PgValue::Polygon(
                PgPolygon::from_sql(ty, raw)?
                    .0
                    .into_iter()
                    .map(point_to_hashable_f64s)
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::POLYGON_ARRAY => Ok(PgValue::PolygonArray(
                Vec::<PgPolygon>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.0.into_iter().map(point_to_hashable_f64s).collect())
                    .collect::<Vec<Vec<_>>>(),
            )),

            &tokio_postgres::types::Type::CIDR => {
                let cidr = IpCidr::from_sql(ty, raw)?;
                Ok(PgValue::Cidr(cidr.to_string()))
            }
            &tokio_postgres::types::Type::CIDR_ARRAY => Ok(PgValue::CidrArray(
                Vec::<IpCidr>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<String>>(),
            )),
            &tokio_postgres::types::Type::INET => {
                let inet = IpAddr::from_sql(ty, raw)?;
                Ok(PgValue::Inet(inet.to_string()))
            }
            &tokio_postgres::types::Type::INET_ARRAY => Ok(PgValue::InetArray(
                Vec::<IpAddr>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<String>>(),
            )),

            &tokio_postgres::types::Type::MACADDR => {
                Ok(PgValue::Macaddr(MacAddressEui48::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::MACADDR_ARRAY => Ok(PgValue::MacaddrArray(
                Vec::<MacAddressEui48>::from_sql(ty, raw)?,
            )),
            &tokio_postgres::types::Type::MACADDR8 => {
                Ok(PgValue::Macaddr8(MacAddressEui64::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::MACADDR8_ARRAY => Ok(PgValue::Macaddr8Array(
                Vec::<MacAddressEui64>::from_sql(ty, raw)?,
            )),
            &tokio_postgres::types::Type::DATE => {
                Ok(PgValue::Date(naive_to_date(NaiveDate::from_sql(ty, raw)?)))
            }
            &tokio_postgres::types::Type::DATE_ARRAY => Ok(PgValue::DateArray(
                Vec::<NaiveDate>::from_sql(ty, raw)?
                    .into_iter()
                    .map(naive_to_date)
                    .collect::<Vec<Date>>(),
            )),

            &tokio_postgres::types::Type::DATE_RANGE
            | &tokio_postgres::types::Type::DATE_RANGE_ARRAY
            | &tokio_postgres::types::Type::DATEMULTI_RANGE => {
                Err("date ranges are not yet supported".into())
            }

            &tokio_postgres::types::Type::TIME => {
                Ok(PgValue::Time(naive_to_time(NaiveTime::from_sql(ty, raw)?)))
            }
            &tokio_postgres::types::Type::TIME_ARRAY => Ok(PgValue::TimeArray(
                Vec::<NaiveTime>::from_sql(ty, raw)?
                    .into_iter()
                    .map(naive_to_time)
                    .collect::<Vec<Time>>(),
            )),
            &tokio_postgres::types::Type::TIMESTAMP => Ok(PgValue::Timestamp(
                naive_to_timestamp(NaiveDateTime::from_sql(ty, raw)?),
            )),
            &tokio_postgres::types::Type::TIMESTAMP_ARRAY => Ok(PgValue::TimestampArray(
                Vec::<NaiveDateTime>::from_sql(ty, raw)?
                    .into_iter()
                    .map(naive_to_timestamp)
                    .collect::<Vec<Timestamp>>(),
            )),

            &tokio_postgres::types::Type::INTERVAL
            | &tokio_postgres::types::Type::INTERVAL_ARRAY => {
                Err("intervals are not yet supported".into())
            }

            &tokio_postgres::types::Type::TIMETZ | &tokio_postgres::types::Type::TIMETZ_ARRAY => {
                Err("timetz is not supported".into())
            }

            &tokio_postgres::types::Type::TS_RANGE
            | &tokio_postgres::types::Type::TS_RANGE_ARRAY
            | &tokio_postgres::types::Type::TSMULTI_RANGE
            | &tokio_postgres::types::Type::TSMULTI_RANGE_ARRAY => {
                Err("timestamp ranges are not yet supported".into())
            }

            &tokio_postgres::types::Type::TIMESTAMPTZ => Ok(PgValue::TimestampTz(
                utc_to_timestamptz(DateTime::<Utc>::from_sql(ty, raw)?),
            )),
            &tokio_postgres::types::Type::TIMESTAMPTZ_ARRAY => Ok(PgValue::TimestampTzArray(
                Vec::<DateTime<Utc>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(utc_to_timestamptz)
                    .collect::<Vec<TimestampTz>>(),
            )),

            &tokio_postgres::types::Type::TSTZ_RANGE
            | &tokio_postgres::types::Type::TSTZ_RANGE_ARRAY
            | &tokio_postgres::types::Type::TSTZMULTI_RANGE => {
                Err("timestamptz ranges are not yet supported".into())
            }

            &tokio_postgres::types::Type::UUID => {
                Ok(PgValue::Uuid(Uuid::from_sql(ty, raw)?.to_string()))
            }
            &tokio_postgres::types::Type::UUID_ARRAY => Ok(PgValue::UuidArray(
                Vec::<Uuid>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<String>>(),
            )),
            &tokio_postgres::types::Type::PG_LSN => {
                Ok(PgValue::PgLsn(PgLsn::from_sql(ty, raw)?.into()))
            }
            &tokio_postgres::types::Type::PG_LSN_ARRAY => Ok(PgValue::PgLsnArray(
                Vec::<PgLsn>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.into())
                    .collect::<Vec<u64>>(),
            )),

            // Extension types (matched by name since they have no built-in Type constant)
            t if t.name() == "hstore" => {
                let map = HashMap::<String, Option<String>>::from_sql(ty, raw)?;
                Ok(PgValue::Hstore(map.into_iter().collect()))
            }

            // All other types are unsupported
            t => Err(format!(
                "unsupported type [{t}], consider using a cast like 'value'::string or 'value'::jsonb"
            )
            .into()),
        }
    }

    fn accepts(_ty: &PgType) -> bool {
        // Accept all types and provide specific error messages in from_sql
        true
    }

    fn from_sql_null(_ty: &PgType) -> Result<Self, Box<dyn Error + Sync + Send>> {
        Ok(PgValue::Null)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn bind(value: &PgValue, ty: &PgType) -> Result<(), Box<dyn Error + Sync + Send>> {
        value.to_sql_checked(ty, &mut BytesMut::new()).map(|_| ())
    }

    #[test]
    fn scalar_bound_to_a_different_type_is_refused() {
        // Same width, different meaning: used to be read as `5e-324`.
        let err = bind(&PgValue::Int8(1), &PgType::FLOAT8).unwrap_err();
        assert!(err.is::<WrongType>(), "{err}");
        assert!(err.to_string().contains("float8"), "{err}");
        // Different width: used to fail with an error that names no type.
        assert!(bind(&PgValue::Int4(1), &PgType::INT8).is_err());
        assert!(bind(&PgValue::Int8(1), &PgType::INT4).is_err());
        assert!(bind(&PgValue::Int8(1), &PgType::TIMESTAMPTZ).is_err());
        assert!(bind(&PgValue::Int4(1), &PgType::DATE).is_err());
        assert!(bind(&PgValue::Bool(true), &PgType::INT2).is_err());
        assert!(bind(&PgValue::Date(Date::Ymd((2000, 1, 2))), &PgType::INT4).is_err());
        assert!(
            bind(
                &PgValue::Time(Time {
                    hour: 1,
                    min: 2,
                    sec: 3,
                    micro: 4
                }),
                &PgType::INT8
            )
            .is_err()
        );
    }

    #[test]
    fn text_is_refused_for_fixed_width_types_only() {
        for ty in [
            PgType::INT8,
            PgType::FLOAT8,
            PgType::UUID,
            PgType::NUMERIC,
            PgType::BOOL,
        ] {
            assert!(
                bind(&PgValue::Text("12345678".into()), &ty).is_err(),
                "{ty}"
            );
        }
        for ty in [PgType::TEXT, PgType::VARCHAR, PgType::JSON, PgType::UNKNOWN] {
            assert!(bind(&PgValue::Text("x".into()), &ty).is_ok(), "{ty}");
        }
    }

    #[test]
    fn string_and_byte_values_bind_only_to_types_that_read_them() {
        let regclass = PgType::REGCLASS;
        let int8 = PgType::INT8;
        for (value, ty) in [
            (PgValue::Text("abcd".into()), &regclass),
            (PgValue::Text("abcd".into()), &PgType::XID),
            (PgValue::Text("abcdefgh".into()), &PgType::PG_LSN),
            (PgValue::Text("abcdef".into()), &PgType::MACADDR),
            (PgValue::Text("abcdefgh".into()), &PgType::XID8),
            (PgValue::Text("abcdefgh".into()), &PgType::MACADDR8),
            (PgValue::Text("abcdef".into()), &PgType::TID),
            (PgValue::Text("abcdefghabcdefgh".into()), &PgType::POINT),
            (PgValue::Text("{}".into()), &PgType::JSONB),
            (PgValue::Name("12345678".into()), &int8),
            (PgValue::Varchar((None, b"12345678".to_vec())), &int8),
            (PgValue::Char((8, b"12345678".to_vec())), &int8),
            (PgValue::Xml("12345678".into()), &int8),
            (PgValue::Bytea(b"12345678".to_vec()), &int8),
            (PgValue::Bytea(b"abc".to_vec()), &PgType::TEXT),
            (PgValue::Json("1234".into()), &PgType::INT4),
            (PgValue::Jsonb("1234".into()), &PgType::TEXT),
        ] {
            let err = bind(&value, ty).unwrap_err();
            assert!(err.is::<WrongType>(), "{value:?} as {ty}: {err}");
            assert!(err.to_string().contains(ty.name()), "{err}");
        }

        let citext = PgType::new("citext".into(), 0, Kind::Simple, "public".into());
        let mood = PgType::new(
            "mood".into(),
            0,
            Kind::Enum(vec!["happy".into()]),
            "public".into(),
        );
        let label = PgType::new(
            "label".into(),
            0,
            Kind::Domain(PgType::TEXT),
            "public".into(),
        );
        for ty in [
            PgType::TEXT,
            PgType::VARCHAR,
            PgType::BPCHAR,
            PgType::NAME,
            PgType::UNKNOWN,
            PgType::JSON,
            PgType::XML,
            citext,
            mood,
            label,
        ] {
            for value in [
                PgValue::Text("happy".into()),
                PgValue::Name("happy".into()),
                PgValue::Varchar((None, b"happy".to_vec())),
                PgValue::Char((5, b"happy".to_vec())),
                PgValue::Xml("happy".into()),
            ] {
                assert!(bind(&value, &ty).is_ok(), "{value:?} as {ty}");
            }
        }
        assert!(bind(&PgValue::Bytea(vec![0, 255]), &PgType::BYTEA).is_ok());
        for ty in [PgType::JSON, PgType::JSONB] {
            assert!(bind(&PgValue::Json("{}".into()), &ty).is_ok(), "{ty}");
            assert!(bind(&PgValue::Jsonb("{}".into()), &ty).is_ok(), "{ty}");
        }
    }

    #[test]
    fn string_values_bind_to_char_refcursor_and_ltree_types() {
        for ty in [PgType::CHAR, PgType::REFCURSOR] {
            for value in [
                PgValue::Text("r".into()),
                PgValue::Name("r".into()),
                PgValue::Varchar((None, b"r".to_vec())),
                PgValue::Char((1, b"r".to_vec())),
                PgValue::Xml("r".into()),
            ] {
                assert!(bind(&value, &ty).is_ok(), "{value:?} as {ty}");
            }
        }

        // `&str` writes the version byte that the `ltree` types start with,
        // but the raw bytes of a `varchar` or `char` value lack it.
        let ltree = PgType::new("ltree".into(), 0, Kind::Simple, "public".into());
        for value in [
            PgValue::Text("a.b".into()),
            PgValue::Name("a.b".into()),
            PgValue::Xml("a.b".into()),
        ] {
            let mut out = BytesMut::new();
            value.to_sql_checked(&ltree, &mut out).unwrap();
            assert_eq!(&out[..], b"\x01a.b", "{value:?}");
        }
        for value in [
            PgValue::Varchar((None, b"a.b".to_vec())),
            PgValue::Char((3, b"a.b".to_vec())),
        ] {
            let err = bind(&value, &ltree).unwrap_err();
            assert!(err.is::<WrongType>(), "{value:?}: {err}");
        }
    }

    #[test]
    fn values_bound_to_a_domain_are_encoded_as_its_base_type() {
        let doc = PgType::new(
            "doc".into(),
            0,
            Kind::Domain(PgType::JSONB),
            "public".into(),
        );
        let mut out = BytesMut::new();
        PgValue::Jsonb("{}".into())
            .to_sql_checked(&doc, &mut out)
            .unwrap();
        assert_eq!(&out[..], b"\x01{}");

        let ltree = PgType::new("ltree".into(), 0, Kind::Simple, "public".into());
        let label_path = PgType::new("label_path".into(), 0, Kind::Domain(ltree), "public".into());
        let mut out = BytesMut::new();
        PgValue::Text("a.b".into())
            .to_sql_checked(&label_path, &mut out)
            .unwrap();
        assert_eq!(&out[..], b"\x01a.b");
    }

    #[test]
    fn text_encoded_bit_and_array_values_bind_only_to_types_that_read_them() {
        for (value, ty) in [
            (PgValue::TsQuery("abcdefgh".into()), PgType::INT8),
            (PgValue::PgSnapshot((1, 2, vec![])), PgType::INT8),
            (PgValue::Bit((8, vec![0; 8])), PgType::INT8),
            (PgValue::Varbit((None, vec![0; 8])), PgType::INT8),
            (
                PgValue::TextArray(vec!["abcdefgh".into()]),
                PgType::INT8_ARRAY,
            ),
            (
                PgValue::VarcharArray(vec![(None, b"abcdefgh".to_vec())]),
                PgType::INT8_ARRAY,
            ),
            (PgValue::ByteaArray(vec![vec![0; 8]]), PgType::INT8_ARRAY),
            (PgValue::Int8Array(vec![1]), PgType::FLOAT8_ARRAY),
            (PgValue::JsonbArray(vec!["{}".into()]), PgType::TEXT_ARRAY),
        ] {
            let err = bind(&value, &ty).unwrap_err();
            assert!(err.is::<WrongType>(), "{value:?} as {ty}: {err}");
            assert!(err.to_string().contains(ty.name()), "{err}");
        }

        for (value, ty) in [
            (PgValue::TsQuery("a & b".into()), PgType::TEXT),
            (PgValue::PgSnapshot((1, 2, vec![])), PgType::TEXT),
            (PgValue::TextArray(vec!["a".into()]), PgType::TEXT_ARRAY),
            (
                PgValue::VarcharArray(vec![(None, b"a".to_vec())]),
                PgType::VARCHAR_ARRAY,
            ),
            (PgValue::Int4Array(vec![1]), PgType::OID_ARRAY),
            (PgValue::JsonbArray(vec!["{}".into()]), PgType::JSONB_ARRAY),
        ] {
            assert!(bind(&value, &ty).is_ok(), "{value:?} as {ty}");
        }
    }

    #[test]
    fn matching_scalars_still_bind() {
        assert!(bind(&PgValue::Int8(1), &PgType::INT8).is_ok());
        assert!(bind(&PgValue::BigSerial(1), &PgType::INT8).is_ok());
        assert!(bind(&PgValue::Int4(1), &PgType::INT4).is_ok());
        assert!(bind(&PgValue::Int2(1), &PgType::INT2).is_ok());
        assert!(bind(&PgValue::Bool(true), &PgType::BOOL).is_ok());
        assert!(bind(&PgValue::Date(Date::Ymd((2000, 1, 2))), &PgType::DATE).is_ok());
        assert!(bind(&PgValue::Null, &PgType::INT8).is_ok());
    }

    #[test]
    fn int4_binds_to_oid_like_types() {
        for ty in [PgType::OID, PgType::REGCLASS, PgType::REGTYPE] {
            assert!(bind(&PgValue::Int4(1259), &ty).is_ok(), "{ty}");
        }
        assert!(bind(&PgValue::Int8(1259), &PgType::OID).is_err());
    }

    #[test]
    fn array_bound_to_a_shallower_type_is_refused_not_a_panic() {
        let err = bind(&PgValue::Int4Array(vec![1]), &PgType::INT4).unwrap_err();
        assert!(err.is::<WrongType>(), "{err}");
        assert!(bind(&PgValue::TextArray(vec!["x".into()]), &PgType::TEXT).is_err());
        assert!(bind(&PgValue::Int2Vector(vec![1]), &PgType::INT2).is_err());
        assert!(
            bind(
                &PgValue::Int2VectorArray(vec![vec![1]]),
                &PgType::INT2_ARRAY
            )
            .is_err()
        );

        assert!(bind(&PgValue::Int4Array(vec![1]), &PgType::INT4_ARRAY).is_ok());
        assert!(bind(&PgValue::Int2Vector(vec![1]), &PgType::INT2_VECTOR).is_ok());
        assert!(
            bind(
                &PgValue::Int2VectorArray(vec![vec![1]]),
                &PgType::INT2_VECTOR_ARRAY
            )
            .is_ok()
        );
    }

    #[test]
    fn domain_is_checked_as_its_base_type() {
        let domain = PgType::new(
            "positive_id".into(),
            0,
            Kind::Domain(PgType::INT8),
            "public".into(),
        );
        assert!(bind(&PgValue::Int8(1), &domain).is_ok());
        assert!(bind(&PgValue::Int4(1), &domain).is_err());
    }

    type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

    fn round_trip<T: ToSql>(
        value: PgValue,
        expected: PgValue,
        ty: PgType,
        native: &T,
    ) -> TestResult {
        let mut wire = BytesMut::new();
        assert!(matches!(native.to_sql(&ty, &mut wire)?, IsNull::No));
        let mut encoded = BytesMut::new();
        assert!(matches!(
            value.to_sql_checked(&ty, &mut encoded)?,
            IsNull::No
        ));
        assert!(encoded == wire, "encoding a value as {ty}");
        let decoded = PgValue::from_sql(&ty, &wire)?;
        assert!(
            format!("{decoded:?}") == format!("{expected:?}"),
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
        let time =
            NaiveTime::from_hms_micro_opt(23, 59, 58, 123456).ok_or("invalid fixture time")?;
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
    fn paths_and_polygons_match_postgres_wire_values() -> TestResult {
        let points = vec![
            (0.5_f64.integer_decode(), (-1.0_f64).integer_decode()),
            (2.0_f64.integer_decode(), 3.25_f64.integer_decode()),
        ];
        let line = LineString::<f64>::from(vec![(0.5, -1.0), (2.0, 3.25)]);
        round_trip(
            PgValue::Path(points.clone()),
            PgValue::Path(points.clone()),
            PgType::PATH,
            &line,
        )?;
        round_trip(
            PgValue::PathArray(vec![points.clone()]),
            PgValue::PathArray(vec![points.clone()]),
            PgType::PATH_ARRAY,
            &vec![line],
        )?;

        // polygon_send: the point count, then each point's x and y.
        let mut polygon = BytesMut::new();
        polygon.put_i32(2);
        for v in [0.5_f64, -1.0, 2.0, 3.25] {
            polygon.put_f64(v);
        }
        let mut encoded = BytesMut::new();
        PgValue::Polygon(points.clone()).to_sql_checked(&PgType::POLYGON, &mut encoded)?;
        assert_eq!(encoded, polygon);
        assert_eq!(
            format!("{:?}", PgValue::from_sql(&PgType::POLYGON, &polygon)?),
            format!("{:?}", PgValue::Polygon(points.clone()))
        );
        let array = PgValue::PolygonArray(vec![points.clone()]);
        let mut encoded = BytesMut::new();
        array.to_sql_checked(&PgType::POLYGON_ARRAY, &mut encoded)?;
        assert_eq!(
            format!("{:?}", PgValue::from_sql(&PgType::POLYGON_ARRAY, &encoded)?),
            format!("{array:?}")
        );

        for (value, ty) in [
            (PgValue::Path(points.clone()), PgType::POINT_ARRAY),
            (PgValue::Polygon(points.clone()), PgType::PATH),
            (PgValue::PathArray(vec![points.clone()]), PgType::PATH),
            (PgValue::Polygon(vec![]), PgType::POLYGON),
        ] {
            assert!(bind(&value, &ty).is_err(), "invalid value accepted as {ty}");
        }
        for raw in [
            &polygon[..3],
            &polygon[..polygon.len() - 1],
            &[255, 255, 255, 255],
        ] {
            assert!(PgValue::from_sql(&PgType::POLYGON, raw).is_err());
        }
        Ok(())
    }

    #[test]
    fn path_columns_decode_closed_or_open_and_refuse_a_wrong_point_count() -> TestResult {
        let points = vec![
            (0.5_f64.integer_decode(), (-1.0_f64).integer_decode()),
            (2.0_f64.integer_decode(), 3.25_f64.integer_decode()),
        ];
        // path_send: whether it is closed, the point count, then each point.
        for closed in [0, 1] {
            let mut path = BytesMut::new();
            path.put_u8(closed);
            path.put_i32(2);
            for v in [0.5_f64, -1.0, 2.0, 3.25] {
                path.put_f64(v);
            }
            assert_eq!(
                format!("{:?}", PgValue::from_sql(&PgType::PATH, &path)?),
                format!("{:?}", PgValue::Path(points.clone())),
                "closed={closed}"
            );
            assert!(PgValue::from_sql(&PgType::PATH, &path[..path.len() - 1]).is_err());
        }
        // A count that is negative, or past the points that follow it, used to
        // panic or abort the host while room was made for it.
        for raw in [
            &[1][..],
            &[1, 255, 255, 255, 255],
            &[1, 127, 255, 255, 255],
            &[0, 0, 0, 0, 1],
        ] {
            let err = PgValue::from_sql(&PgType::PATH, raw)
                .err()
                .ok_or("a malformed path decoded")?;
            assert!(err.to_string().contains("path"), "{err}");
        }
        Ok(())
    }

    #[test]
    fn lseg_parameters_match_postgres_wire_values_and_lines_are_refused() -> TestResult {
        let lseg = (
            (1.0_f64.integer_decode(), 2.0_f64.integer_decode()),
            (3.0_f64.integer_decode(), (-4.5_f64).integer_decode()),
        );
        // lseg_recv: the first point's x and y, then the second's.
        let mut wire = BytesMut::new();
        for v in [1.0_f64, 2.0, 3.0, -4.5] {
            wire.put_f64(v);
        }
        let mut encoded = BytesMut::new();
        PgValue::Lseg(lseg).to_sql_checked(&PgType::LSEG, &mut encoded)?;
        assert_eq!(encoded, wire);
        let mut encoded = BytesMut::new();
        PgValue::LsegArray(vec![lseg, lseg]).to_sql_checked(&PgType::LSEG_ARRAY, &mut encoded)?;
        assert!(encoded.ends_with(&wire), "{encoded:?}");

        for (value, ty) in [
            (PgValue::Line(lseg), PgType::LINE),
            (PgValue::LineArray(vec![lseg]), PgType::LINE_ARRAY),
        ] {
            let err = bind(&value, &ty).unwrap_err();
            assert!(err.to_string().contains("line & line[]"), "{err}");
        }
        Ok(())
    }

    #[test]
    fn fixed_width_values_are_refused_for_other_types_as_wide() {
        let point = (1.5_f64.integer_decode(), (-2.5_f64).integer_decode());
        let uuid = Uuid::from_u128(123).to_string();
        for (value, own, others) in [
            (
                PgValue::Point(point),
                PgType::POINT,
                vec![PgType::UUID, PgType::INTERVAL],
            ),
            (
                PgValue::Uuid(uuid),
                PgType::UUID,
                vec![PgType::POINT, PgType::TEXT],
            ),
            (
                PgValue::Box((point, point)),
                PgType::BOX,
                vec![PgType::LSEG],
            ),
            (
                PgValue::Lseg((point, point)),
                PgType::LSEG,
                vec![PgType::BOX, PgType::PATH],
            ),
            (
                PgValue::PgLsn(1),
                PgType::PG_LSN,
                vec![PgType::INT8, PgType::FLOAT8],
            ),
        ] {
            assert!(bind(&value, &own).is_ok(), "{value:?} as {own}");
            for ty in others {
                let err = bind(&value, &ty).unwrap_err();
                assert!(err.is::<WrongType>(), "{value:?} as {ty}: {err}");
            }
        }
        // Text by another name is refused where text is.
        for value in [
            PgValue::Name("12345678".into()),
            PgValue::Xml("12345678".into()),
            PgValue::Char((8, b"12345678".to_vec())),
            PgValue::Varchar((None, b"12345678".to_vec())),
        ] {
            assert!(bind(&value, &PgType::INT8).is_err(), "{value:?}");
            assert!(bind(&value, &PgType::TEXT).is_ok(), "{value:?}");
        }
    }

    #[test]
    fn fixed_width_arrays_are_refused_for_other_element_types() {
        let point = (1.5_f64.integer_decode(), (-2.5_f64).integer_decode());
        let uuid = Uuid::from_u128(123).to_string();
        for (value, own, other) in [
            (
                PgValue::UuidArray(vec![uuid]),
                PgType::UUID_ARRAY,
                PgType::INTERVAL_ARRAY,
            ),
            (
                PgValue::PointArray(vec![point]),
                PgType::POINT_ARRAY,
                PgType::UUID_ARRAY,
            ),
            (
                PgValue::BoxArray(vec![(point, point)]),
                PgType::BOX_ARRAY,
                PgType::LSEG_ARRAY,
            ),
            (
                PgValue::LsegArray(vec![(point, point)]),
                PgType::LSEG_ARRAY,
                PgType::BOX_ARRAY,
            ),
            (
                PgValue::PgLsnArray(vec![1]),
                PgType::PG_LSN_ARRAY,
                PgType::INT8_ARRAY,
            ),
            (
                PgValue::PathArray(vec![vec![point]]),
                PgType::PATH_ARRAY,
                PgType::POLYGON_ARRAY,
            ),
            (
                PgValue::PolygonArray(vec![vec![point]]),
                PgType::POLYGON_ARRAY,
                PgType::PATH_ARRAY,
            ),
        ] {
            assert!(bind(&value, &own).is_ok(), "{value:?} as {own}");
            let err = bind(&value, &other).unwrap_err();
            assert!(err.is::<WrongType>(), "{value:?} as {other}: {err}");
        }
    }

    #[test]
    fn domain_over_an_array_is_checked_as_its_base_array() {
        let domain = PgType::new(
            "intarr".into(),
            0,
            Kind::Domain(PgType::INT4_ARRAY),
            "public".into(),
        );
        assert!(bind(&PgValue::Int4Array(vec![1]), &domain).is_ok());
        assert!(bind(&PgValue::Int8Array(vec![1]), &domain).is_err());
        assert!(bind(&PgValue::Int4(1), &domain).is_err());
    }

    #[test]
    fn numeric_exponents_are_rescaled_or_refused() -> TestResult {
        let ten = PgNumeric::new(pg_bigdecimal::BigDecimal::parse_bytes(b"10", 10));
        for text in ["1e1", "1.0e1", "10"] {
            let mut encoded = BytesMut::new();
            PgValue::Numeric(text.into()).to_sql_checked(&PgType::NUMERIC, &mut encoded)?;
            let mut wire = BytesMut::new();
            ten.to_sql(&PgType::NUMERIC, &mut wire)?;
            assert_eq!(encoded, wire, "{text}");
        }
        bind(
            &PgValue::NumericArray(vec!["1e1000".into(), "-2.5e3".into()]),
            &PgType::NUMERIC_ARRAY,
        )?;
        for text in [
            "1e1001".to_string(),
            "1e9223372036854775807".to_string(),
            "1".repeat(MAX_NUMERIC_TEXT_LEN + 1),
        ] {
            assert!(bind(&PgValue::Numeric(text), &PgType::NUMERIC).is_err());
        }
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
            PgType::PATH,
            PgType::PATH_ARRAY,
            PgType::POINT,
            PgType::POINT_ARRAY,
            PgType::POLYGON,
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
                "invalid value accepted as {ty}"
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
            assert!(
                linestring_to_hashable_f64s_tuple(LineString::new(vec![point; length])).is_err()
            );
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
}
