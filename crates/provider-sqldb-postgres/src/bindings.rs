//! This module contains generated bindings, and code to make bindings more ergonomic
//!

use core::net::IpAddr;
use std::collections::HashMap;
use std::error::Error;
use std::str::FromStr;

use anyhow::{bail, Context as _};
use bigdecimal::num_traits::Float;
use bit_vec::BitVec;
use bytes::Bytes;
use bytes::{BufMut, BytesMut};
use chrono::{
    DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Offset as _, Timelike,
    Utc,
};
use cidr::IpCidr;
use geo_types::{coord, LineString, Point, Rect};
use pg_bigdecimal::PgNumeric;
use postgres_types::{FromSql, IsNull, PgLsn, ToSql, Type as PgType};
use tokio_postgres::Row;
use uuid::Uuid;

// Bindgen happens here
wit_bindgen_wrpc::generate!({
  with: {
      "wasmcloud:postgres/types@0.1.0-draft": generate,
      "wasmcloud:postgres/query@0.1.0-draft": generate,
      "wasmcloud:postgres/prepared@0.1.0-draft": generate,
  },
});

// Start bindgen-generated type imports
pub(crate) use exports::wasmcloud::postgres::prepared;
pub(crate) use exports::wasmcloud::postgres::query;

pub(crate) use query::{PgValue, QueryError, ResultRow};

pub(crate) use prepared::{
    PreparedStatementExecError, PreparedStatementToken, StatementPrepareError,
};

use crate::bindings::wasmcloud::postgres::types::{
    Date, HashableF64, MacAddressEui48, MacAddressEui64, Numeric, Offset, ResultRowEntry, Time,
    Timestamp, TimestampTz,
};
// End of bindgen-generated type imports

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

/// Convert [`Rect`] which *should* contain points for two opposite corners into
/// a tuple of points (AKA two `HashableF64`s)
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

/// Convert a [`Linestring`] which represents a line/line segment and is expected to
/// contain only *two* points, into a tuple of `HashableF64`s
fn linestring_to_hashable_f64s_tuple(
    l: LineString<f64>,
) -> anyhow::Result<((HashableF64, HashableF64), (HashableF64, HashableF64))> {
    match linestring_to_hashable_f64s(l)[..] {
        [start, end] => Ok((start, end)),
        _ => bail!("unexpected number of points in line string"),
    }
}

/// Convert a [`Linestring`] into a vector of points (AKA two bindgen-generated `HashableF64`s)
fn linestring_to_hashable_f64s(l: LineString<f64>) -> Vec<(HashableF64, HashableF64)> {
    l.into_points()
        .into_iter()
        .map(point_to_hashable_f64s)
        .collect::<Vec<_>>()
}

/// Convert a [`Point`] into two bindgen-generated `HashableF64`s
fn point_to_hashable_f64s(p: Point<f64>) -> (HashableF64, HashableF64) {
    let (x, y) = p.x_y();
    (x.integer_decode(), y.integer_decode())
}

impl MacAddressEui48 {
    fn as_bytes(&self) -> [u8; 6] {
        [
            self.bytes.0,
            self.bytes.1,
            self.bytes.2,
            self.bytes.3,
            self.bytes.4,
            self.bytes.5,
        ]
    }
}

impl MacAddressEui64 {
    fn as_bytes(&self) -> [u8; 8] {
        [
            self.bytes.0,
            self.bytes.1,
            self.bytes.2,
            self.bytes.3,
            self.bytes.4,
            self.bytes.5,
            self.bytes.6,
            self.bytes.7,
        ]
    }
}

impl From<MacAddressEui48> for PgValue {
    fn from(m: MacAddressEui48) -> PgValue {
        PgValue::Macaddr(m)
    }
}

impl From<MacAddressEui64> for PgValue {
    fn from(m: MacAddressEui64) -> PgValue {
        PgValue::Macaddr8(m)
    }
}

impl TryFrom<&Date> for chrono::NaiveDate {
    type Error = anyhow::Error;

    fn try_from(d: &Date) -> anyhow::Result<NaiveDate> {
        match d {
            Date::PositiveInfinity => Ok(NaiveDate::MAX),
            Date::NegativeInfinity => Ok(NaiveDate::MAX),
            Date::Ymd((year, month, day)) => NaiveDate::from_ymd_opt(*year, *month, *day)
                .with_context(|| format!("failed to build date from ymd ({year}/{month}/{day})")),
        }
    }
}

impl From<NaiveDate> for Date {
    fn from(nd: NaiveDate) -> Date {
        match nd {
            NaiveDate::MAX => Date::PositiveInfinity,
            NaiveDate::MIN => Date::NegativeInfinity,
            nd => Date::Ymd((nd.year(), nd.month(), nd.day())),
        }
    }
}

impl TryFrom<&Time> for NaiveTime {
    type Error = anyhow::Error;

    fn try_from(
        Time {
            hour,
            min,
            sec,
            micro,
        }: &Time,
    ) -> anyhow::Result<NaiveTime> {
        NaiveTime::from_hms_micro_opt(*hour, *min, *sec, *micro)
            .with_context(|| format!("failed to convert time [{hour}h {min}m {sec}s {micro}micro]"))
    }
}

impl From<NaiveTime> for Time {
    fn from(nt: NaiveTime) -> Time {
        Time {
            hour: nt.hour(),
            min: nt.minute(),
            sec: nt.second(),
            micro: nt.nanosecond() / 1_000,
        }
    }
}

impl TryFrom<&Timestamp> for NaiveDateTime {
    type Error = anyhow::Error;

    fn try_from(Timestamp { date, time }: &Timestamp) -> anyhow::Result<NaiveDateTime> {
        match (date, time) {
            (Date::NegativeInfinity, _) | (Date::PositiveInfinity, _) => {
                bail!("negative/positive infinite date times are not supported")
            }
            (Date::Ymd(_), time) => Ok(NaiveDateTime::new(date.try_into()?, time.try_into()?)),
        }
    }
}

impl From<NaiveDateTime> for Timestamp {
    fn from(ndt: NaiveDateTime) -> Timestamp {
        Timestamp {
            date: ndt.date().into(),
            time: ndt.time().into(),
        }
    }
}

impl TryFrom<&Offset> for FixedOffset {
    type Error = anyhow::Error;

    fn try_from(timezone: &Offset) -> anyhow::Result<FixedOffset> {
        match timezone {
            Offset::EasternHemisphereSecs(secs) => FixedOffset::east_opt(*secs)
                .with_context(|| format!("failed to convert eastern hemisphere seconds [{secs}]")),
            Offset::WesternHemisphereSecs(secs) => FixedOffset::west_opt(*secs)
                .with_context(|| format!("failed to convert western hemisphere seconds [{secs}]")),
        }
    }
}

impl TryFrom<&TimestampTz> for DateTime<Utc> {
    type Error = anyhow::Error;

    fn try_from(
        TimestampTz { timestamp, offset }: &TimestampTz,
    ) -> anyhow::Result<chrono::DateTime<Utc>> {
        let fixed_offset: FixedOffset = offset.try_into()?;
        let timestamp: NaiveDateTime = timestamp.try_into()?;
        Ok(
            chrono::DateTime::<FixedOffset>::from_naive_utc_and_offset(timestamp, fixed_offset)
                .into(),
        )
    }
}

impl From<DateTime<Utc>> for TimestampTz {
    fn from(dt: DateTime<Utc>) -> TimestampTz {
        TimestampTz {
            offset: Offset::WesternHemisphereSecs(dt.offset().fix().local_minus_utc()),
            timestamp: dt.naive_local().into(),
        }
    }
}

/// Build a `ResultRow` from a [`Row`]
pub(crate) fn into_result_row(r: Row) -> ResultRow {
    let mut rr = Vec::new();
    for (idx, col) in r.columns().iter().enumerate() {
        rr.push(ResultRowEntry {
            column_name: col.name().into(),
            value: r.get(idx),
        });
    }
    rr
}

impl ToSql for MacAddressEui48 {
    fn to_sql(
        &self,
        ty: &PgType,
        out: &mut BytesMut,
    ) -> core::result::Result<IsNull, Box<dyn Error + Sync + Send>> {
        match ty {
            &tokio_postgres::types::Type::MACADDR => {
                out.put_slice(&self.as_bytes());
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
    ) -> core::result::Result<IsNull, Box<dyn Error + Sync + Send>> {
        match ty {
            &tokio_postgres::types::Type::MACADDR => {
                out.put_slice(&self.as_bytes());
                Ok(IsNull::No)
            }
            _ => Err("invalid Postgres type for EUI64 MAC address".into()),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        matches!(ty, &tokio_postgres::types::Type::MACADDR)
    }

    tokio_postgres::types::to_sql_checked!();
}

impl ToSql for PgValue {
    fn to_sql(
        &self,
        ty: &PgType,
        out: &mut BytesMut,
    ) -> core::result::Result<IsNull, Box<dyn Error + Sync + Send>> {
        match self {
            PgValue::Null => Ok(IsNull::Yes),
            // Numeric
            PgValue::BigInt(n) | PgValue::Int8(n) => n.to_sql(ty, out),
            PgValue::Int8Array(ns) => ns.to_sql(ty, out),
            PgValue::BigSerial(n) | PgValue::Serial8(n) => n.to_sql(ty, out),
            PgValue::Bool(n) | PgValue::Boolean(n) => n.to_sql(ty, out),
            PgValue::BoolArray(ns) => ns.to_sql(ty, out),
            PgValue::Double(d)
            | PgValue::Float8(d) => {
                f64_from_tuple(d).to_sql(ty, out)
            }
            PgValue::Float8Array(ds) => {
                ds.iter().map(f64_from_tuple).collect::<Vec<_>>().to_sql(ty, out)
            }
            PgValue::Real(d)
            | PgValue::Float4(d) => {
                f64_from_tuple(d).to_sql(ty, out)
            }
            PgValue::Float4Array(ds) => {
                ds.iter().map(f64_from_tuple).collect::<Vec<_>>().to_sql(ty, out)
            }
            PgValue::Integer(n) | PgValue::Int(n) | PgValue::Int4(n) => n.to_sql(ty, out),
            PgValue::Int4Array(ns) => ns.to_sql(ty, out),
            PgValue::Numeric(s)
            | PgValue::Decimal(s)
            // Money (use is discouraged)
            //
            // fractional precision is determined by the database's `lc_monetary` setting.
            //
            // NOTE: if you are storing currency amounts, consider
            // using integer (whole number) counts of smallest indivisible pieces of currency
            // (ex. cent amounts to represent United States Dollars; 100 cents = 1 USD)
            | PgValue::Money(s) => {
                let bigd = pg_bigdecimal::BigDecimal::parse_bytes(s.as_bytes(), 10).ok_or_else(|| {
                    format!("failed to parse bigint [{s}]")
                })?;
                PgNumeric::new(Some(bigd)).to_sql(ty, out)
            }
            PgValue::NumericArray(ss) | PgValue::MoneyArray(ss) => {
                ss.
                    iter()
                    .map(|s| {
                        pg_bigdecimal::BigDecimal::parse_bytes(s.as_bytes(), 10)
                            .map(|v| PgNumeric::new(Some(v)))
                            .ok_or_else(|| {
                                format!("failed to parse bigint [{s}]")
                            })
                    })
                    .collect::<Result<Vec<PgNumeric>, _>>()?
                    .to_sql(ty, out)
            }

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
                bytes.as_ref().to_sql(ty, out)
            }
            PgValue::BitArray(many_bits) => {
                let mut vec: Vec<Vec<u8>> = Vec::new();
                for (exact_size, bytes) in many_bits.iter() {
                    if bytes.len() != *exact_size as usize {
                        return Err("bitfield size does not match".into());
                    }
                    vec.push(bytes.to_vec())
                }
                vec.to_sql(ty, out)
            }
            PgValue::BitVarying((limit, bytes)) | PgValue::Varbit((limit, bytes)) => {
                if limit.is_some_and(|limit| bytes.len() > limit as usize) {
                    return Err("bit field length is greater than limit".into());
                }
                bytes.as_ref().to_sql(ty, out)
            }
            PgValue::VarbitArray(many_varbits) => {
                let mut valid_varbits: Vec<Vec<u8>> = Vec::new();
                for (limit, bytes) in many_varbits {
                    if limit.is_some_and(|limit| bytes.len() > limit as usize) {
                        return Err("bit field length is greater than limit".into());
                    }
                    valid_varbits.push(bytes.to_vec())
                }
                valid_varbits.to_sql(ty, out)

            }
            PgValue::Bytea(bytes) => bytes.as_ref().to_sql(ty, out),
            PgValue::ByteaArray(many_bytes) => many_bytes.into_iter().map(AsRef::as_ref).collect::<Vec<_>>().to_sql(ty, out),

            // Characters
            PgValue::Char((len, bytes)) => {
                if bytes.len() != *len as usize {
                    return Err("char length does not match specified size".into());
                }
                bytes.as_ref().to_sql(ty, out)
            }
            PgValue::CharArray(many_chars) => {
                let mut valid_chars = Vec::new();
                for (len, bytes) in many_chars {
                    if bytes.len() != *len as usize {
                        return Err("char length does not match specified size".into());
                    }
                    valid_chars.push(bytes.as_ref());
                }
                valid_chars.to_sql(ty, out)
            }
            PgValue::Varchar((maybe_len, bytes)) => {
                if let Some(limit) = maybe_len {
                    if bytes.len() > *limit as usize {
                        return Err(format!(
                            "char length [{}] does not match specified limit [{limit}]",
                            bytes.len(),
                        )
                        .into());
                    }
                }
                bytes.as_ref().to_sql(ty, out)
            }
            PgValue::VarcharArray(vs) => {
                let mut valid_varchars = Vec::new();
                for (maybe_len, bytes) in vs {
                    if let Some(limit) = maybe_len {
                        if bytes.len() > *limit as usize {
                            return Err(format!(
                                "char length [{}] does not match specified limit [{limit}]",
                                bytes.len(),
                            )
                                       .into());
                        }
                    }
                    valid_varchars.push(bytes.as_ref())
                }
                valid_varchars.to_sql(ty, out)
            }

            // Networking
            PgValue::Cidr(cidr) => {
                IpCidr::from_str(cidr)
                    .map_err(|e| format!("invalid cidr: {e}"))?
                    .to_sql(ty, out)
            }
            PgValue::CidrArray(cidrs) => {
                cidrs
                    .iter()
                    .map(|v| IpCidr::from_str(v).map_err(|e| format!("invalid cidr: {e}")))
                    .collect::<Result<Vec<IpCidr>, _>>()?
                    .to_sql(ty, out)
            }

            PgValue::Inet(addr) => {
                IpAddr::from_str(addr)
                    .map_err(|e| format!("invalid address: {e}"))?
                    .to_sql(ty, out)
            }
            PgValue::InetArray(inets) => {
                inets
                    .iter()
                    .map(|v| IpAddr::from_str(v).map_err(|e| format!("invalid address: {e}")))
                    .collect::<Result<Vec<IpAddr>, _>>()?
                    .to_sql(ty, out)

            }

            // EUI-48 (octets)
            PgValue::Macaddr(m) => {
                m.to_sql(ty, out)
            }
            PgValue::MacaddrArray(macs) => {
                macs.clone().to_sql(ty, out)
            }

            // EUI-64 (deprecated)
            PgValue::Macaddr8(m) => {
                m.to_sql(ty, out)
            }
            PgValue::Macaddr8Array(macs) => {
                macs.clone().to_sql(ty, out)
            }

            // Geo
            PgValue::Circle(_) | PgValue::CircleArray(_) => {
                Err("circle & circle[] are not supported".into())
            },
            PgValue::Box(((start_x, start_y), (end_x, end_y)))  => {
                let start_x = f64_from_tuple(start_x);
                let start_y = f64_from_tuple(start_y);
                let end_x = f64_from_tuple(end_x);
                let end_y = f64_from_tuple(end_y);
                Rect::<f64>::new(
                    coord! { x: start_x, y: start_y },
                    coord! { x: end_x, y: end_y },
                ).to_sql(ty, out)
            }

            PgValue::Line(((start_x, start_y), (end_x, end_y))) | PgValue::Lseg(((start_x, start_y), (end_x, end_y))) => {
                LineString::<f64>::new(vec![
                    coord!{ x: f64_from_tuple(start_x), y: f64_from_tuple(start_y) },
                    coord!{ x: f64_from_tuple(end_x), y: f64_from_tuple(end_y) },
                ]).to_sql(ty, out)
            },
            PgValue::LineArray(lines) | PgValue::LsegArray(lines) => {
                lines
                    .iter()
                    .map(|((start_x, start_y), (end_x, end_y))| LineString::<f64>::new(vec![
                        coord! { x: f64_from_tuple(start_x), y: f64_from_tuple(start_y) },
                        coord! { x: f64_from_tuple(end_x), y: f64_from_tuple(end_y) },
                    ]))
                    .collect::<Vec<LineString>>()
                    .to_sql(ty, out)
            }

            PgValue::BoxArray(boxes) => {
                boxes
                    .iter()
                    .map(|((start_x, start_y), (end_x, end_y))| Rect::<f64>::new(
                        coord! { x: f64_from_tuple(start_x), y: f64_from_tuple(start_y) },
                        coord! { x: f64_from_tuple(end_x), y: f64_from_tuple(end_y) },
                    ))
                    .collect::<Vec<Rect<f64>>>()
                    .to_sql(ty, out)

            }

            PgValue::Point((x, y)) => {
                Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y)).to_sql(ty, out)
            },
            PgValue::PointArray(points) => {
                points.iter().map(|(x, y)| Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y))).collect::<Vec<Point>>().to_sql(ty, out)
            }

            PgValue::Path(points) | PgValue::Polygon(points) => {
                if points.is_empty() { return Err("invalid polygon, no points specified".into()) }
                points
                    .iter()
                    .map(|(x, y)|  Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y)))
                    .collect::<Vec<Point<f64>>>()
                    .to_sql(ty, out)
            },

            PgValue::PathArray(paths) | PgValue::PolygonArray(paths) => {
                paths
                    .iter()
                    .map(|path| {
                        path
                            .iter()
                            .map(|(x, y)|  Point::<f64>::new(f64_from_tuple(x), f64_from_tuple(y)))
                            .collect::<Vec<Point<f64>>>()
                    })
                    .collect::<Vec<Vec<Point<f64>>>>()
                    .to_sql(ty, out)
            }


            // Date-time
            PgValue::Date(d) => {
                let d: NaiveDate = d.try_into()?;
                d.to_sql(ty, out)
            }
            PgValue::DateArray(ds) => {
                ds
                    .iter()
                    .map(|v| v.try_into())
                    .collect::<Result<Vec<NaiveDate>, _>>()?
                    .to_sql(ty, out)
            }

            PgValue::Interval(_) | PgValue::IntervalArray(_) => {
                Err("interval not supported (consider using a cast like 'value'::text::interval)".into())
            },

            PgValue::Time(t) => {
                let t: chrono::NaiveTime = t.try_into()?;
                t.to_sql(ty, out)
            }
            PgValue::TimeArray(ts) => {
                ts
                    .iter()
                    .map(|t| t.try_into())
                    .collect::<Result<Vec<NaiveTime>, _>>()?
                    .to_sql(ty, out)
            }

            PgValue::TimeTz(_) | PgValue::TimeTzArray(_) => {
                Err("timetz not supported (consider using a cast like 'value'::text::timetz)".into())
            }

            PgValue::Timestamp(ts) => {
                let ts: NaiveDateTime = ts.try_into()?;
                ts.to_sql(ty, out)
            }
            PgValue::TimestampArray(tss) => {
                tss
                    .iter()
                    .map(|ts| ts.try_into())
                    .collect::<Result<Vec<NaiveDateTime>, _>>()?
                    .to_sql(ty, out)
            }

            PgValue::TimestampTz(tstz) => {
                let tstz: chrono::DateTime<Utc> = tstz.try_into()?;
                tstz.to_sql(ty, out)
            }
            PgValue::TimestampTzArray(tstzs) => {
                tstzs
                    .iter()
                    .map(|tstz| tstz.try_into())
                    .collect::<Result<Vec<chrono::DateTime<Utc>>, _>>()?
                    .to_sql(ty, out)
            }

            // JSON
            PgValue::Json(s) | PgValue::Jsonb(s) => {
                serde_json::Value::from_str(s)
                    .map_err(|e| format!("failed to parse JSON: {e}"))?
                    .to_sql(ty, out)
            },
            PgValue::JsonArray(json_strings) | PgValue::JsonbArray(json_strings) => {
                json_strings
                    .iter()
                    .map(|s| serde_json::Value::from_str(s))
                    .collect::<Result<Vec<serde_json::Value>, _>>()?
                    .to_sql(ty, out)
            },
            // Postgres-internal
            PgValue::PgLsn(offset) => PgLsn::from(*offset).to_sql(ty, out),
            PgValue::PgLsnArray(offsets) => {
                offsets
                    .iter()
                    .cloned()
                    .map(PgLsn::from)
                    .collect::<Vec<PgLsn>>()
                    .to_sql(ty, out)
            }
            PgValue::PgSnapshot((xmin, xmax, xip_list)) => {
                format!(
                    "{xmin}:{xmax}:{}",
                    xip_list
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<String>>()
                        .join(","),
                ).to_sql(ty, out)
            },
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
            PgValue::TsVector(_) => {
                Err("tsvector not supported (consider using a cast like 'value'::text::tsvector)".into())
            }

            // UUIDs
            PgValue::Uuid(s) => {
                Uuid::from_str(s)?.to_sql(ty, out)
            }
            PgValue::UuidArray(ss) => {
                ss
                    .iter()
                    .map(|v| Uuid::from_str(v.as_ref()))
                    .collect::<Result<Vec<Uuid>, _>>()?
                    .to_sql(ty, out)
            }

            // UUIDs
            PgValue::Hstore(h) => {
                let map  = HashMap::<String, Option<String>>::from_iter(h.iter().cloned());
                map.to_sql(ty, out)
            }
        }
    }

    fn accepts(_ty: &PgType) -> bool {
        // NOTE: we don't actually support all types, but pretend to, in order to
        // use more specific/tailored error messages
        true
    }

    tokio_postgres::types::to_sql_checked!();
}

impl TryFrom<&[u8]> for MacAddressEui48 {
    type Error = Box<dyn Error + Sync + Send>;

    fn try_from(bytes: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match bytes[..] {
            [octet0, octet1, octet2, octet3, octet4, octet5] => Ok(Self {
                bytes: (octet0, octet1, octet2, octet3, octet4, octet5),
            }),
            _ => Err(format!(
                "unexpected number of bytes ({}) in EUI48 mac address",
                bytes.len()
            )
            .into()),
        }
    }
}

impl TryFrom<&[u8]> for MacAddressEui64 {
    type Error = Box<dyn Error + Sync + Send>;

    fn try_from(bytes: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match bytes[..] {
            [octet0, octet1, octet2, octet3, octet4, octet5, octet6, octet7] => Ok(Self {
                bytes: (
                    octet0, octet1, octet2, octet3, octet4, octet5, octet6, octet7,
                ),
            }),
            _ => Err(format!(
                "unexpected number of bytes ({}) in EUI64 mac address",
                bytes.len()
            )
            .into()),
        }
    }
}

impl FromSql<'_> for MacAddressEui48 {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match (ty, raw) {
            (&tokio_postgres::types::Type::MACADDR, bytes) if bytes.len() == 6 => {
                MacAddressEui48::try_from(bytes)
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
                MacAddressEui64::try_from(bytes)
            }
            _ => Err("invalid type/raw input for EUI64 MAC address".into()),
        }
    }

    fn accepts(ty: &PgType) -> bool {
        matches!(ty, &tokio_postgres::types::Type::MACADDR8)
    }
}

impl FromSql<'_> for PgValue {
    fn from_sql(ty: &PgType, raw: &[u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        match ty {
            &tokio_postgres::types::Type::BOOL => Ok(PgValue::Bool(bool::from_sql(ty, raw)?)),
            &tokio_postgres::types::Type::BOOL_ARRAY => {
                Ok(PgValue::BoolArray(Vec::<bool>::from_sql(ty, raw)?))
            }
            &tokio_postgres::types::Type::BYTEA => {
                let buf = Vec::<u8>::from_sql(ty, raw)?;
                Ok(PgValue::Bytea(buf.into()))
            }
            &tokio_postgres::types::Type::BYTEA_ARRAY => {
                let buf = Vec::<Vec<u8>>::from_sql(ty, raw)?;
                Ok(PgValue::ByteaArray(buf.into_iter().map(Bytes::from).collect()))
            }
            &tokio_postgres::types::Type::CHAR => {
                let s = Vec::<u8>::from_sql(ty, raw)?;
                let len = s.len().try_into()?;
                Ok(PgValue::Char((len, s.into())))
            }
            &tokio_postgres::types::Type::CHAR_ARRAY => {
                let list = Vec::<Vec<u8>>::from_sql(ty, raw)?;
                let mut cs = Vec::new();
                for c in list {
                    cs.push((c.len().try_into()?, c.into()));
                }
                Ok(PgValue::CharArray(cs))
            }
            &tokio_postgres::types::Type::BIT => {
                let vec = BitVec::from_sql(ty, raw)?;
                let len = vec.len().try_into()?;
                Ok(PgValue::Bit((len, vec.to_bytes().into())))
            }
            &tokio_postgres::types::Type::BIT_ARRAY => {
                let vecs = Vec::<BitVec>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.len().try_into().map(|len| (len, v.to_bytes().into())))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(PgValue::BitArray(vecs))
            }
            &tokio_postgres::types::Type::VARBIT => {
                let vec = BitVec::from_sql(ty, raw)?;
                let len = vec.len().try_into()?;
                Ok(PgValue::Bit((len, vec.to_bytes().into())))
            }
            &tokio_postgres::types::Type::VARBIT_ARRAY => {
                let varbits = Vec::<BitVec>::from_sql(ty, raw)?
                    .into_iter()
                    // NOTE: we don't know what the  limit of this varbit was if we only
                    // have the bytes, default to allowing it to be unbounded
                    .map(|v| (None, v.to_bytes().into()))
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
            &tokio_postgres::types::Type::JSONB => Ok(PgValue::Json(
                serde_json::Value::from_sql(ty, raw)?.to_string(),
            )),
            &tokio_postgres::types::Type::JSONB_ARRAY => Ok(PgValue::JsonbArray(
                Vec::<serde_json::Value>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::VARCHAR => {
                Ok(PgValue::Varchar(
                    // We cannot know whether the varchar had a limit
                    Vec::<u8>::from_sql(ty, raw).map(|v| (None, v.into()))?,
                ))
            }
            &tokio_postgres::types::Type::VARCHAR_ARRAY => Ok(PgValue::VarcharArray(
                Vec::<Vec<u8>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|s| (None, s.into()))
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
            &tokio_postgres::types::Type::BOX => {
                Ok(PgValue::Box(rect_to_hashable_f64s(Rect::<f64>::from_sql(
                    ty, raw,
                )?)))
            }
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
                Vec::<Point<f64>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(point_to_hashable_f64s)
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::PATH_ARRAY => Ok(PgValue::PathArray(
                Vec::<Vec<Point<f64>>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|points| points.into_iter().map(point_to_hashable_f64s).collect())
                    .collect::<Vec<_>>(),
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
                Vec::<Point<f64>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(point_to_hashable_f64s)
                    .collect::<Vec<_>>(),
            )),
            &tokio_postgres::types::Type::POLYGON_ARRAY => Ok(PgValue::PolygonArray(
                Vec::<Vec<Point<f64>>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.into_iter().map(point_to_hashable_f64s).collect())
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
                Ok(MacAddressEui48::from_sql(ty, raw)?.into())
            }
            &tokio_postgres::types::Type::MACADDR_ARRAY => Ok(PgValue::MacaddrArray(
                Vec::<MacAddressEui48>::from_sql(ty, raw)?
            )),
            &tokio_postgres::types::Type::MACADDR8 => {
                Ok(MacAddressEui64::from_sql(ty, raw)?.into())
            }
            &tokio_postgres::types::Type::MACADDR8_ARRAY => Ok(PgValue::Macaddr8Array(
                Vec::<MacAddressEui64>::from_sql(ty, raw)?
            )),
            &tokio_postgres::types::Type::DATE => {
                Ok(PgValue::Date(NaiveDate::from_sql(ty, raw)?.into()))
            }
            &tokio_postgres::types::Type::DATE_ARRAY => {
                Ok(PgValue::DateArray(Vec::<NaiveDate>::from_sql(ty, raw)?
                   .into_iter()
                   .map(|t| t.into())
                   .collect::<Vec<Date>>()))
            }

            &tokio_postgres::types::Type::DATE_RANGE
            | &tokio_postgres::types::Type::DATE_RANGE_ARRAY
            | &tokio_postgres::types::Type::DATEMULTI_RANGE => {
                Err("date ranges are not yet supported".into())
            }

            &tokio_postgres::types::Type::TIME => {
                Ok(PgValue::Time(NaiveTime::from_sql(ty, raw)?.into()))
            }
            &tokio_postgres::types::Type::TIME_ARRAY => {
                Ok(PgValue::TimeArray(Vec::<NaiveTime>::from_sql(ty, raw)?
                   .into_iter()
                   .map(|t| t.into())
                   .collect::<Vec<Time>>()))
            }
            &tokio_postgres::types::Type::TIMESTAMP => {
                Ok(PgValue::Timestamp(NaiveDateTime::from_sql(ty, raw)?.into()))

            },
             &tokio_postgres::types::Type::TIMESTAMP_ARRAY => {
                Ok(PgValue::TimestampArray(Vec::<NaiveDateTime>::from_sql(ty, raw)?
                   .into_iter()
                   .map(|t| t.into())
                   .collect::<Vec<Timestamp>>()))
             }

            &tokio_postgres::types::Type::INTERVAL
            | &tokio_postgres::types::Type::INTERVAL_ARRAY => {
                Err("intervals are not yet supported".into())
            }

            &tokio_postgres::types::Type::TIMETZ |
            &tokio_postgres::types::Type::TIMETZ_ARRAY => {
                Err("timetz is not supported".into())
            }

            &tokio_postgres::types::Type::TS_RANGE
            | &tokio_postgres::types::Type::TS_RANGE_ARRAY
            | &tokio_postgres::types::Type::TSMULTI_RANGE
            | &tokio_postgres::types::Type::TSMULTI_RANGE_ARRAY => {
                Err("timestamp ranges are not yet supported".into())
            }

            &tokio_postgres::types::Type::TIMESTAMPTZ => {
                Ok(PgValue::TimestampTz(DateTime::<Utc>::from_sql(ty, raw)?.into()))
            }
            &tokio_postgres::types::Type::TIMESTAMPTZ_ARRAY => Ok(PgValue::TimestampTzArray(
                Vec::<DateTime<Utc>>::from_sql(ty, raw)?
                    .into_iter()
                    .map(|v| v.into())
                    .collect::<Vec<TimestampTz>>()
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

            // All other types are unsupported
            t => Err(format!("unsupported type [{}], consider using a cast like 'value'::string or 'value'::jsonb", t).into()),
        }
    }

    fn accepts(_ty: &PgType) -> bool {
        // NOTE: we don't actually support all types, but pretend to, in order to
        // use a more specific error message that encourages using a cast where possible
        true
    }

    fn from_sql_null(_ty: &PgType) -> Result<Self, Box<dyn Error + Sync + Send>> {
        Ok(PgValue::Null)
    }
}
