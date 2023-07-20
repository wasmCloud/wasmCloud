use chrono::{DateTime, NaiveDateTime, Utc};
use minicbor::{encode::Write, Encoder};
use tokio_postgres::{types::Type, Row};

/// encode query result into CBOR arroy-of-arrays
pub(crate) fn encode_rows<W>(
    enc: &mut Encoder<W>,
    rows: &[Row],
) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write,
    <W as minicbor::encode::Write>::Error: std::error::Error + 'static,
{
    enc.array(rows.len() as u64)?;
    for row in rows.iter() {
        enc.array(row.len() as u64)?;
        for (i, col) in row.columns().iter().enumerate() {
            // TODO: check col.kind() to see if it's an array
            //   then load with array_from_sql()
            encode_val(enc, row, col.type_(), i)?
        }
    }
    Ok(())
}

/// cbor encode a single value based on the database column type
#[inline]
fn encode_val<'r, W>(
    enc: &mut Encoder<W>,
    row: &'r Row,
    ty: &Type,
    i: usize,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write,
    <W as minicbor::encode::Write>::Error: std::error::Error + 'static,
{
    match *ty {
        // basic types
        Type::BOOL => enc.bool(row.get(i)),
        Type::CHAR => enc.i16(row.get(i)),
        Type::INT2 => enc.i16(row.get(i)),
        Type::INT4 => enc.i32(row.get(i)),
        Type::INT8 => enc.i64(row.get(i)),
        Type::OID => enc.u32(row.get(i)),
        Type::FLOAT4 => enc.f32(row.get(i)),
        Type::FLOAT8 => enc.f64(row.get(i)),

        // Strings
        Type::CHAR_ARRAY
        | Type::VARCHAR
        | Type::TEXT
        | Type::NAME
        | Type::UNKNOWN
        | Type::JSON
        | Type::XID
        | Type::CID
        | Type::XML => enc.str(row.get(i)),

        // byte array
        Type::BYTEA => enc.bytes(row.get(i)),

        // convert uuid to string
        Type::UUID => enc.str(&row.get::<'r, usize, uuid::Uuid>(i).to_string()),

        // timestamp as iso3339 string in UTC
        Type::TIMESTAMP | Type::TIMESTAMPTZ => {
            if let Some(ndt) = NaiveDateTime::from_timestamp_opt(row.get(i), 0) {
                enc.str(&DateTime::<Utc>::from_utc(ndt, Utc).to_string())
            } else {
                enc.str("")
            }
        }
        // date and time to string
        Type::DATE => enc.str(&row.get::<'r, usize, chrono::NaiveDate>(i).to_string()),

        Type::TIME => enc.str(&row.get::<'r, usize, chrono::NaiveTime>(i).to_string()),

        // ip address as string
        Type::INET => enc.str(&row.get::<'r, usize, std::net::IpAddr>(i).to_string()),

        // convert bit vector to bytes
        Type::BIT | Type::VARBIT => enc.bytes(&row.get::<'r, usize, bit_vec::BitVec>(i).to_bytes()),

        // anything else - encode as byte array
        _ => enc.bytes(row.get(i)),
    }?;
    Ok(())
}

/*
    -- these are here as a checklist of types not yet supported --

   /// NAME - 63-byte type for storing system identifiers
   pub const NAME: Type = Type(Inner::Name);

   /// INT2VECTOR - array of int2, used in system tables
   pub const INT2_VECTOR: Type = Type(Inner::Int2Vector);


   /// REGPROC - registered procedure
   pub const REGPROC: Type = Type(Inner::Regproc);


   /// OIDVECTOR - array of oids, used in system tables
   pub const OID_VECTOR: Type = Type(Inner::OidVector);

   /// PG_DDL_COMMAND - internal type for passing CollectedCommand
   pub const PG_DDL_COMMAND: Type = Type(Inner::PgDdlCommand);

   /// XML&#91;&#93;
   pub const XML_ARRAY: Type = Type(Inner::XmlArray);

   /// PG_NODE_TREE - string representing an internal node tree
   pub const PG_NODE_TREE: Type = Type(Inner::PgNodeTree);

   /// JSON&#91;&#93;
   pub const JSON_ARRAY: Type = Type(Inner::JsonArray);

   /// TABLE_AM_HANDLER
   pub const TABLE_AM_HANDLER: Type = Type(Inner::TableAmHandler);

   /// XID8&#91;&#93;
   pub const XID8_ARRAY: Type = Type(Inner::Xid8Array);

   /// INDEX_AM_HANDLER - pseudo-type for the result of an index AM handler function
   pub const INDEX_AM_HANDLER: Type = Type(Inner::IndexAmHandler);

   /// POINT - geometric point &#39;&#40;x, y&#41;&#39;
   pub const POINT: Type = Type(Inner::Point);

   /// LSEG - geometric line segment &#39;&#40;pt1,pt2&#41;&#39;
   pub const LSEG: Type = Type(Inner::Lseg);

   /// PATH - geometric path &#39;&#40;pt1,...&#41;&#39;
   pub const PATH: Type = Type(Inner::Path);

   /// BOX - geometric box &#39;&#40;lower left,upper right&#41;&#39;
   pub const BOX: Type = Type(Inner::Box);

   /// POLYGON - geometric polygon &#39;&#40;pt1,...&#41;&#39;
   pub const POLYGON: Type = Type(Inner::Polygon);

   /// LINE - geometric line
   pub const LINE: Type = Type(Inner::Line);

   /// LINE&#91;&#93;
   pub const LINE_ARRAY: Type = Type(Inner::LineArray);

   /// CIDR - network IP address/netmask, network address
   pub const CIDR: Type = Type(Inner::Cidr);

   /// CIDR&#91;&#93;
   pub const CIDR_ARRAY: Type = Type(Inner::CidrArray);


   /// UNKNOWN - pseudo-type representing an undetermined type
   pub const UNKNOWN: Type = Type(Inner::Unknown);

   /// CIRCLE - geometric circle &#39;&#40;center,radius&#41;&#39;
   pub const CIRCLE: Type = Type(Inner::Circle);

   /// CIRCLE&#91;&#93;
   pub const CIRCLE_ARRAY: Type = Type(Inner::CircleArray);

   /// MACADDR8 - XX:XX:XX:XX:XX:XX:XX:XX, MAC address
   pub const MACADDR8: Type = Type(Inner::Macaddr8);

   /// MACADDR8&#91;&#93;
   pub const MACADDR8_ARRAY: Type = Type(Inner::Macaddr8Array);

   /// MONEY - monetary amounts, &#36;d,ddd.cc
   pub const MONEY: Type = Type(Inner::Money);

   /// MONEY&#91;&#93;
   pub const MONEY_ARRAY: Type = Type(Inner::MoneyArray);

   /// MACADDR - XX:XX:XX:XX:XX:XX, MAC address
   pub const MACADDR: Type = Type(Inner::Macaddr);

   /// INET - IP address/netmask, host address, netmask optional

   /// BOOL&#91;&#93;
   pub const BOOL_ARRAY: Type = Type(Inner::BoolArray);

   /// NAME&#91;&#93;
   pub const NAME_ARRAY: Type = Type(Inner::NameArray);

   /// INT2&#91;&#93;
   pub const INT2_ARRAY: Type = Type(Inner::Int2Array);

   /// INT2VECTOR&#91;&#93;
   pub const INT2_VECTOR_ARRAY: Type = Type(Inner::Int2VectorArray);

   /// INT4&#91;&#93;
   pub const INT4_ARRAY: Type = Type(Inner::Int4Array);

   /// REGPROC&#91;&#93;
   pub const REGPROC_ARRAY: Type = Type(Inner::RegprocArray);

   /// TEXT&#91;&#93;
   pub const TEXT_ARRAY: Type = Type(Inner::TextArray);

   /// TID&#91;&#93;
   pub const TID_ARRAY: Type = Type(Inner::TidArray);

   /// XID&#91;&#93;
   pub const XID_ARRAY: Type = Type(Inner::XidArray);

   /// CID&#91;&#93;
   pub const CID_ARRAY: Type = Type(Inner::CidArray);

   /// OIDVECTOR&#91;&#93;
   pub const OID_VECTOR_ARRAY: Type = Type(Inner::OidVectorArray);

   /// BPCHAR&#91;&#93;
   pub const BPCHAR_ARRAY: Type = Type(Inner::BpcharArray);

   /// VARCHAR&#91;&#93;
   pub const VARCHAR_ARRAY: Type = Type(Inner::VarcharArray);

   /// INT8&#91;&#93;
   pub const INT8_ARRAY: Type = Type(Inner::Int8Array);

   /// POINT&#91;&#93;
   pub const POINT_ARRAY: Type = Type(Inner::PointArray);

   /// LSEG&#91;&#93;
   pub const LSEG_ARRAY: Type = Type(Inner::LsegArray);

   /// PATH&#91;&#93;
   pub const PATH_ARRAY: Type = Type(Inner::PathArray);

   /// BOX&#91;&#93;
   pub const BOX_ARRAY: Type = Type(Inner::BoxArray);

   /// FLOAT4&#91;&#93;
   pub const FLOAT4_ARRAY: Type = Type(Inner::Float4Array);

   /// FLOAT8&#91;&#93;
   pub const FLOAT8_ARRAY: Type = Type(Inner::Float8Array);

   /// POLYGON&#91;&#93;
   pub const POLYGON_ARRAY: Type = Type(Inner::PolygonArray);

   /// OID&#91;&#93;
   pub const OID_ARRAY: Type = Type(Inner::OidArray);

   /// ACLITEM - access control list
   pub const ACLITEM: Type = Type(Inner::Aclitem);

   /// ACLITEM&#91;&#93;
   pub const ACLITEM_ARRAY: Type = Type(Inner::AclitemArray);

   /// MACADDR&#91;&#93;
   pub const MACADDR_ARRAY: Type = Type(Inner::MacaddrArray);

   /// INET&#91;&#93;
   pub const INET_ARRAY: Type = Type(Inner::InetArray);

   /// BPCHAR - char&#40;length&#41;, blank-padded string, fixed storage length
   pub const BPCHAR: Type = Type(Inner::Bpchar);

   /// DATE - date
   pub const DATE: Type = Type(Inner::Date);

   /// TIME - time of day
   pub const TIME: Type = Type(Inner::Time);

   /// TIMESTAMP - date and time
   pub const TIMESTAMP: Type = Type(Inner::Timestamp);

   /// TIMESTAMP&#91;&#93;
   pub const TIMESTAMP_ARRAY: Type = Type(Inner::TimestampArray);

   /// DATE&#91;&#93;
   pub const DATE_ARRAY: Type = Type(Inner::DateArray);

   /// TIME&#91;&#93;
   pub const TIME_ARRAY: Type = Type(Inner::TimeArray);

   /// TIMESTAMPTZ - date and time with time zone
   pub const TIMESTAMPTZ: Type = Type(Inner::Timestamptz);

   /// TIMESTAMPTZ&#91;&#93;
   pub const TIMESTAMPTZ_ARRAY: Type = Type(Inner::TimestamptzArray);

   /// INTERVAL - &#64; &lt;number&gt; &lt;units&gt;, time interval
   pub const INTERVAL: Type = Type(Inner::Interval);

   /// INTERVAL&#91;&#93;
   pub const INTERVAL_ARRAY: Type = Type(Inner::IntervalArray);

   /// NUMERIC&#91;&#93;
   pub const NUMERIC_ARRAY: Type = Type(Inner::NumericArray);

   /// CSTRING&#91;&#93;
   pub const CSTRING_ARRAY: Type = Type(Inner::CstringArray);

   /// TIMETZ - time of day with time zone
   pub const TIMETZ: Type = Type(Inner::Timetz);

   /// TIMETZ&#91;&#93;
   pub const TIMETZ_ARRAY: Type = Type(Inner::TimetzArray);

   /// BIT - fixed-length bit string
   pub const BIT: Type = Type(Inner::Bit);

   /// BIT&#91;&#93;
   pub const BIT_ARRAY: Type = Type(Inner::BitArray);

   /// VARBIT - variable-length bit string
   pub const VARBIT: Type = Type(Inner::Varbit);

   /// VARBIT&#91;&#93;
   pub const VARBIT_ARRAY: Type = Type(Inner::VarbitArray);

   /// NUMERIC - numeric&#40;precision, decimal&#41;, arbitrary precision number
   pub const NUMERIC: Type = Type(Inner::Numeric);

   /// REFCURSOR - reference to cursor &#40;portal name&#41;
   pub const REFCURSOR: Type = Type(Inner::Refcursor);

   /// REFCURSOR&#91;&#93;
   pub const REFCURSOR_ARRAY: Type = Type(Inner::RefcursorArray);

   /// REGPROCEDURE - registered procedure &#40;with args&#41;
   pub const REGPROCEDURE: Type = Type(Inner::Regprocedure);

   /// REGOPER - registered operator
   pub const REGOPER: Type = Type(Inner::Regoper);

   /// REGOPERATOR - registered operator &#40;with args&#41;
   pub const REGOPERATOR: Type = Type(Inner::Regoperator);

   /// REGCLASS - registered class
   pub const REGCLASS: Type = Type(Inner::Regclass);

   /// REGTYPE - registered type
   pub const REGTYPE: Type = Type(Inner::Regtype);

   /// REGPROCEDURE&#91;&#93;
   pub const REGPROCEDURE_ARRAY: Type = Type(Inner::RegprocedureArray);

   /// REGOPER&#91;&#93;
   pub const REGOPER_ARRAY: Type = Type(Inner::RegoperArray);

   /// REGOPERATOR&#91;&#93;
   pub const REGOPERATOR_ARRAY: Type = Type(Inner::RegoperatorArray);

   /// REGCLASS&#91;&#93;
   pub const REGCLASS_ARRAY: Type = Type(Inner::RegclassArray);

   /// REGTYPE&#91;&#93;
   pub const REGTYPE_ARRAY: Type = Type(Inner::RegtypeArray);

   /// RECORD - pseudo-type representing any composite type
   pub const RECORD: Type = Type(Inner::Record);

   /// CSTRING - C-style string
   pub const CSTRING: Type = Type(Inner::Cstring);

   /// ANY - pseudo-type representing any type
   pub const ANY: Type = Type(Inner::Any);

   /// ANYARRAY - pseudo-type representing a polymorphic array type
   pub const ANYARRAY: Type = Type(Inner::Anyarray);

   /// VOID - pseudo-type for the result of a function with no real result
   pub const VOID: Type = Type(Inner::Void);

   /// TRIGGER - pseudo-type for the result of a trigger function
   pub const TRIGGER: Type = Type(Inner::Trigger);

   /// LANGUAGE_HANDLER - pseudo-type for the result of a language handler function
   pub const LANGUAGE_HANDLER: Type = Type(Inner::LanguageHandler);

   /// INTERNAL - pseudo-type representing an internal data structure
   pub const INTERNAL: Type = Type(Inner::Internal);

   /// ANYELEMENT - pseudo-type representing a polymorphic base type
   pub const ANYELEMENT: Type = Type(Inner::Anyelement);

   /// RECORD&#91;&#93;
   pub const RECORD_ARRAY: Type = Type(Inner::RecordArray);

   /// ANYNONARRAY - pseudo-type representing a polymorphic base type that is not an array
   pub const ANYNONARRAY: Type = Type(Inner::Anynonarray);

   /// TXID_SNAPSHOT&#91;&#93;
   pub const TXID_SNAPSHOT_ARRAY: Type = Type(Inner::TxidSnapshotArray);

   /// UUID&#91;&#93;
   pub const UUID_ARRAY: Type = Type(Inner::UuidArray);

   /// TXID_SNAPSHOT - txid snapshot
   pub const TXID_SNAPSHOT: Type = Type(Inner::TxidSnapshot);

   /// FDW_HANDLER - pseudo-type for the result of an FDW handler function
   pub const FDW_HANDLER: Type = Type(Inner::FdwHandler);

   /// PG_LSN - PostgreSQL LSN datatype
   pub const PG_LSN: Type = Type(Inner::PgLsn);

   /// PG_LSN&#91;&#93;
   pub const PG_LSN_ARRAY: Type = Type(Inner::PgLsnArray);

   /// TSM_HANDLER - pseudo-type for the result of a tablesample method function
   pub const TSM_HANDLER: Type = Type(Inner::TsmHandler);

   /// PG_NDISTINCT - multivariate ndistinct coefficients
   pub const PG_NDISTINCT: Type = Type(Inner::PgNdistinct);

   /// PG_DEPENDENCIES - multivariate dependencies
   pub const PG_DEPENDENCIES: Type = Type(Inner::PgDependencies);

   /// ANYENUM - pseudo-type representing a polymorphic base type that is an enum
   pub const ANYENUM: Type = Type(Inner::Anyenum);

   /// TSVECTOR - text representation for text search
   pub const TS_VECTOR: Type = Type(Inner::TsVector);

   /// TSQUERY - query representation for text search
   pub const TSQUERY: Type = Type(Inner::Tsquery);

   /// GTSVECTOR - GiST index internal text representation for text search
   pub const GTS_VECTOR: Type = Type(Inner::GtsVector);

   /// TSVECTOR&#91;&#93;
   pub const TS_VECTOR_ARRAY: Type = Type(Inner::TsVectorArray);

   /// GTSVECTOR&#91;&#93;
   pub const GTS_VECTOR_ARRAY: Type = Type(Inner::GtsVectorArray);

   /// TSQUERY&#91;&#93;
   pub const TSQUERY_ARRAY: Type = Type(Inner::TsqueryArray);

   /// REGCONFIG - registered text search configuration
   pub const REGCONFIG: Type = Type(Inner::Regconfig);

   /// REGCONFIG&#91;&#93;
   pub const REGCONFIG_ARRAY: Type = Type(Inner::RegconfigArray);

   /// REGDICTIONARY - registered text search dictionary
   pub const REGDICTIONARY: Type = Type(Inner::Regdictionary);

   /// REGDICTIONARY&#91;&#93;
   pub const REGDICTIONARY_ARRAY: Type = Type(Inner::RegdictionaryArray);

   /// JSONB - Binary JSON
   pub const JSONB: Type = Type(Inner::Jsonb);

   /// JSONB&#91;&#93;
   pub const JSONB_ARRAY: Type = Type(Inner::JsonbArray);

   /// ANYRANGE - pseudo-type representing a range over a polymorphic base type
   pub const ANY_RANGE: Type = Type(Inner::AnyRange);

   /// EVENT_TRIGGER - pseudo-type for the result of an event trigger function
   pub const EVENT_TRIGGER: Type = Type(Inner::EventTrigger);

   /// INT4RANGE - range of integers
   pub const INT4_RANGE: Type = Type(Inner::Int4Range);

   /// INT4RANGE&#91;&#93;
   pub const INT4_RANGE_ARRAY: Type = Type(Inner::Int4RangeArray);

   /// NUMRANGE - range of numerics
   pub const NUM_RANGE: Type = Type(Inner::NumRange);

   /// NUMRANGE&#91;&#93;
   pub const NUM_RANGE_ARRAY: Type = Type(Inner::NumRangeArray);

   /// TSRANGE - range of timestamps without time zone
   pub const TS_RANGE: Type = Type(Inner::TsRange);

   /// TSRANGE&#91;&#93;
   pub const TS_RANGE_ARRAY: Type = Type(Inner::TsRangeArray);

   /// TSTZRANGE - range of timestamps with time zone
   pub const TSTZ_RANGE: Type = Type(Inner::TstzRange);

   /// TSTZRANGE&#91;&#93;
   pub const TSTZ_RANGE_ARRAY: Type = Type(Inner::TstzRangeArray);

   /// DATERANGE - range of dates
   pub const DATE_RANGE: Type = Type(Inner::DateRange);

   /// DATERANGE&#91;&#93;
   pub const DATE_RANGE_ARRAY: Type = Type(Inner::DateRangeArray);

   /// INT8RANGE - range of bigints
   pub const INT8_RANGE: Type = Type(Inner::Int8Range);

   /// INT8RANGE&#91;&#93;
   pub const INT8_RANGE_ARRAY: Type = Type(Inner::Int8RangeArray);

   /// JSONPATH - JSON path
   pub const JSONPATH: Type = Type(Inner::Jsonpath);

   /// JSONPATH&#91;&#93;
   pub const JSONPATH_ARRAY: Type = Type(Inner::JsonpathArray);

   /// REGNAMESPACE - registered namespace
   pub const REGNAMESPACE: Type = Type(Inner::Regnamespace);

   /// REGNAMESPACE&#91;&#93;
   pub const REGNAMESPACE_ARRAY: Type = Type(Inner::RegnamespaceArray);

   /// REGROLE - registered role
   pub const REGROLE: Type = Type(Inner::Regrole);

   /// REGROLE&#91;&#93;
   pub const REGROLE_ARRAY: Type = Type(Inner::RegroleArray);

   /// REGCOLLATION - registered collation
   pub const REGCOLLATION: Type = Type(Inner::Regcollation);

   /// REGCOLLATION&#91;&#93;
   pub const REGCOLLATION_ARRAY: Type = Type(Inner::RegcollationArray);

   /// PG_MCV_LIST - multivariate MCV list
   pub const PG_MCV_LIST: Type = Type(Inner::PgMcvList);

   /// PG_SNAPSHOT - snapshot
   pub const PG_SNAPSHOT: Type = Type(Inner::PgSnapshot);

   /// PG_SNAPSHOT&#91;&#93;
   pub const PG_SNAPSHOT_ARRAY: Type = Type(Inner::PgSnapshotArray);

   /// XID8 - full transaction id
   pub const XID8: Type = Type(Inner::Xid8);

   /// ANYCOMPATIBLE - pseudo-type representing a polymorphic common type
   pub const ANYCOMPATIBLE: Type = Type(Inner::Anycompatible);

   /// ANYCOMPATIBLEARRAY - pseudo-type representing an array of polymorphic common type elements
   pub const ANYCOMPATIBLEARRAY: Type = Type(Inner::Anycompatiblearray);

   /// ANYCOMPATIBLENONARRAY - pseudo-type representing a polymorphic common type that is not an array
   pub const ANYCOMPATIBLENONARRAY: Type = Type(Inner::Anycompatiblenonarray);

   /// ANYCOMPATIBLERANGE - pseudo-type representing a range over a polymorphic common type
   pub const ANYCOMPATIBLE_RANGE: Type = Type(Inner::AnycompatibleRange);

*/
