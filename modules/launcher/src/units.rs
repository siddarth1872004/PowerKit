//! Unit conversion: `10 km to mi`, `72 f in c`, `1.5 gb as mb`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Length,
    Mass,
    Volume,
    Area,
    Time,
    Speed,
    Data,
    Temperature,
}

struct Unit {
    kind: Kind,
    /// Display symbol.
    symbol: &'static str,
    aliases: &'static [&'static str],
    /// Value of one unit in the kind's base unit (temperature: unused).
    factor: f64,
}

const fn u(kind: Kind, symbol: &'static str, aliases: &'static [&'static str], factor: f64) -> Unit {
    Unit {
        kind,
        symbol,
        aliases,
        factor,
    }
}

use Kind::*;

#[rustfmt::skip]
const UNITS: &[Unit] = &[
    u(Length, "mm", &["millimeter", "millimeters", "millimetre", "millimetres"], 0.001),
    u(Length, "cm", &["centimeter", "centimeters", "centimetre", "centimetres"], 0.01),
    u(Length, "m", &["meter", "meters", "metre", "metres"], 1.0),
    u(Length, "km", &["kilometer", "kilometers", "kilometre", "kilometres"], 1000.0),
    u(Length, "in", &["inch", "inches", "\""], 0.0254),
    u(Length, "ft", &["foot", "feet", "'"], 0.3048),
    u(Length, "yd", &["yard", "yards"], 0.9144),
    u(Length, "mi", &["mile", "miles"], 1609.344),
    u(Length, "nmi", &["nauticalmile", "nauticalmiles"], 1852.0),
    u(Mass, "mg", &["milligram", "milligrams"], 0.001),
    u(Mass, "g", &["gram", "grams"], 1.0),
    u(Mass, "kg", &["kilogram", "kilograms", "kilo", "kilos"], 1000.0),
    u(Mass, "t", &["tonne", "tonnes"], 1_000_000.0),
    u(Mass, "oz", &["ounce", "ounces"], 28.349523125),
    u(Mass, "lb", &["lbs", "pound", "pounds"], 453.59237),
    u(Mass, "st", &["stone", "stones"], 6350.29318),
    u(Volume, "ml", &["milliliter", "milliliters", "millilitre", "millilitres"], 0.001),
    u(Volume, "l", &["liter", "liters", "litre", "litres"], 1.0),
    u(Volume, "gal", &["gallon", "gallons"], 3.785411784),
    u(Volume, "qt", &["quart", "quarts"], 0.946352946),
    u(Volume, "pt", &["pint", "pints"], 0.473176473),
    u(Volume, "cup", &["cups"], 0.2365882365),
    u(Volume, "floz", &["fluidounce", "fluidounces"], 0.0295735295625),
    u(Volume, "tbsp", &["tablespoon", "tablespoons"], 0.01478676478125),
    u(Volume, "tsp", &["teaspoon", "teaspoons"], 0.00492892159375),
    u(Area, "m²", &["m2", "sqm"], 1.0),
    u(Area, "km²", &["km2", "sqkm"], 1_000_000.0),
    u(Area, "ft²", &["ft2", "sqft"], 0.09290304),
    u(Area, "ha", &["hectare", "hectares"], 10_000.0),
    u(Area, "acre", &["acres", "ac"], 4046.8564224),
    u(Time, "ms", &["millisecond", "milliseconds"], 0.001),
    u(Time, "s", &["sec", "secs", "second", "seconds"], 1.0),
    u(Time, "min", &["mins", "minute", "minutes"], 60.0),
    u(Time, "h", &["hr", "hrs", "hour", "hours"], 3600.0),
    u(Time, "d", &["day", "days"], 86_400.0),
    u(Time, "wk", &["week", "weeks"], 604_800.0),
    u(Time, "yr", &["year", "years"], 31_557_600.0),
    u(Speed, "m/s", &["mps"], 1.0),
    u(Speed, "km/h", &["kmh", "kph"], 1.0 / 3.6),
    u(Speed, "mph", &["mi/h"], 0.44704),
    u(Speed, "kn", &["knot", "knots", "kt"], 1852.0 / 3600.0),
    u(Data, "B", &["b", "byte", "bytes"], 1.0),
    u(Data, "KB", &["kb", "kilobyte", "kilobytes"], 1e3),
    u(Data, "MB", &["mb", "megabyte", "megabytes"], 1e6),
    u(Data, "GB", &["gb", "gigabyte", "gigabytes"], 1e9),
    u(Data, "TB", &["tb", "terabyte", "terabytes"], 1e12),
    u(Data, "KiB", &["kib", "kibibyte", "kibibytes"], 1024.0),
    u(Data, "MiB", &["mib", "mebibyte", "mebibytes"], 1_048_576.0),
    u(Data, "GiB", &["gib", "gibibyte", "gibibytes"], 1_073_741_824.0),
    u(Data, "TiB", &["tib", "tebibyte", "tebibytes"], 1_099_511_627_776.0),
    u(Temperature, "°C", &["c", "celsius", "°c"], 0.0),
    u(Temperature, "°F", &["f", "fahrenheit", "°f"], 0.0),
    u(Temperature, "K", &["k", "kelvin"], 0.0),
];

fn find(name: &str) -> Option<&'static Unit> {
    let lower = name.to_lowercase();
    UNITS
        .iter()
        .find(|u| u.symbol == name || u.aliases.contains(&name))
        .or_else(|| {
            UNITS
                .iter()
                .find(|u| u.symbol.to_lowercase() == lower || u.aliases.contains(&lower.as_str()))
        })
}

fn to_kelvin(v: f64, symbol: &str) -> f64 {
    match symbol {
        "°C" => v + 273.15,
        "°F" => (v - 32.0) * 5.0 / 9.0 + 273.15,
        _ => v,
    }
}

fn from_kelvin(k: f64, symbol: &str) -> f64 {
    match symbol {
        "°C" => k - 273.15,
        "°F" => (k - 273.15) * 9.0 / 5.0 + 32.0,
        _ => k,
    }
}

/// A finished conversion, for example `10 km` = `6.21371192237 mi`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    pub value: f64,
    pub symbol: &'static str,
    pub input: String,
}

/// Parse `<number> <unit> (to|in|as) <unit>`. The space between number and unit is
/// optional (`10km to mi`).
pub fn convert(query: &str) -> Option<Conversion> {
    let q = query.trim();
    // ASCII lowering keeps byte offsets valid for slicing `q`.
    let lower = q.to_ascii_lowercase();
    let (lhs, rhs) = [" to ", " in ", " as ", " -> "]
        .iter()
        .filter_map(|sep| lower.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i)
        .map(|(i, len)| (q[..i].trim(), q[i + len..].trim()))?;
    let split = lhs
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | '_' | ',')))
        .map(|(i, _)| i)?;
    let number: f64 = lhs[..split].replace(['_', ','], "").parse().ok()?;
    let from = find(lhs[split..].trim())?;
    let to = find(rhs)?;
    if from.kind != to.kind {
        return None;
    }
    let value = if from.kind == Temperature {
        from_kelvin(to_kelvin(number, from.symbol), to.symbol)
    } else {
        number * from.factor / to.factor
    };
    value.is_finite().then(|| Conversion {
        value,
        symbol: to.symbol,
        input: format!("{} {}", crate::calc::format(number), from.symbol),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::format;

    fn conv(q: &str) -> String {
        convert(q)
            .map(|c| format!("{} {}", format(c.value), c.symbol))
            .unwrap_or_else(|| "none".into())
    }

    #[test]
    fn converts_within_a_kind() {
        assert_eq!(conv("10 km to mi"), "6.21371192237 mi");
        assert_eq!(conv("1mi in m"), "1609.344 m");
        assert_eq!(conv("2 lb as kg"), "0.90718474 kg");
        assert_eq!(conv("1 GiB to MB"), "1073.741824 MB");
        assert_eq!(conv("90 min to h"), "1.5 h");
        assert_eq!(conv("100 km/h to mph"), "62.1371192237 mph");
        assert_eq!(conv("1,000 m to km"), "1 km");
    }

    #[test]
    fn temperatures_are_affine() {
        assert_eq!(conv("212 f to c"), "100 °C");
        assert_eq!(conv("-40 °C in °F"), "-40 °F");
        assert_eq!(conv("0 k to c"), "-273.15 °C");
    }

    #[test]
    fn rejects_mismatched_or_unknown() {
        for q in [
            "10 km to kg",
            "10 parsecs to m",
            "km to mi",
            "10 km",
            "how to cook",
            "x to y",
        ] {
            assert_eq!(conv(q), "none", "{q:?}");
        }
    }
}
