//! Text -> numbers for the oxygen, pressure and airflow columns of `conditions`.
//!
//! The text columns are what the paper said. This turns them into numbers in fixed units
//! (percent oxygen, kPa, cm/s) for the numeric columns added in migration 0005.
//!
//! The rule is: never guess. A value counts only when the text states its unit. Text that
//! is ambiguous, states a bound ("below 16%"), lists several separate conditions, or has a
//! number with no unit gives `None`, and the numeric columns stay NULL. One value gives
//! min = max; a stated range gives min and max.

/// One stated value (min == max) or a stated range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

impl Range {
    fn of(a: f64, b: f64) -> Range {
        Range { min: a.min(b), max: a.max(b) }
    }
    pub fn is_single(&self) -> bool {
        self.min == self.max
    }
}

/// The three quantities of one condition row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Numbers {
    pub oxygen_pct: Option<Range>,
    pub pressure_kpa: Option<Range>,
    pub airflow_cm_s: Option<Range>,
}

/// Reads one row's three text columns. Oxygen given as a partial pressure is turned into a
/// percentage only when the same row states one total pressure.
pub fn parse_condition(oxygen: Option<&str>, pressure: Option<&str>, airflow: Option<&str>) -> Numbers {
    let pressure_kpa = pressure.and_then(pressure_kpa);
    Numbers {
        oxygen_pct: oxygen.and_then(|text| oxygen_pct(text, pressure_kpa)),
        pressure_kpa,
        airflow_cm_s: airflow.and_then(airflow_cm_s),
    }
}

/// How two neighbouring numbers in the text are joined.
#[derive(Clone, Copy, PartialEq)]
enum Link {
    Range,
    List,
    Apart,
}

struct Num {
    value: f64,
    /// The text after this number, up to the next number, lower-cased.
    after: String,
    /// How it is joined to the next number.
    link: Link,
}

/// Words that say the number is a limit, not the value tested, or that the value varied.
const BOUNDS: [&str; 20] = [
    "<", ">", "≤", "≥", "below", "above", "lower", "higher", "less than", "more than", "greater than", "up to", "at least", "at most", "under ", "over ",
    "varied", "varies", "various", "variable",
];

fn states_a_bound(text: &str) -> bool {
    let lower = text.to_lowercase();
    BOUNDS.iter().any(|w| lower.contains(w))
}

/// Every number in the text with what follows it. Digits inside a name ("O2", "C4H10") are not numbers.
fn numbers(text: &str) -> Vec<Num> {
    let chars: Vec<char> = text.chars().collect();
    let mut found: Vec<(f64, usize, usize)> = vec![];
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut dot = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && !dot && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())) {
                dot |= chars[i] == '.';
                i += 1;
            }
            let in_name = start > 0 && chars[start - 1].is_alphabetic();
            // "s-1" and "s^-1" are part of a unit, not a value.
            let exponent = start > 0 && matches!(chars[start - 1], '-' | '^' | '−') && start > 1 && (chars[start - 2].is_alphabetic() || chars[start - 2] == '^');
            if !in_name && !exponent {
                if let Ok(value) = chars[start..i].iter().collect::<String>().parse::<f64>() {
                    found.push((value, start, i));
                }
            }
        } else {
            i += 1;
        }
    }
    (0..found.len())
        .map(|k| {
            let end = found.get(k + 1).map(|n| n.1).unwrap_or(chars.len());
            let after: String = chars[found[k].2..end].iter().collect::<String>().to_lowercase();
            let link = if k + 1 < found.len() { link(&after) } else { Link::Apart };
            Num { value: found[k].0, after, link }
        })
        .collect()
}

/// Words that may sit between two numbers of one range or list without separating them.
const BETWEEN: [&str; 33] = [
    "o2", "oxygen", "percent", "mol", "mole", "vol", "volume", "fraction", "approx", "approximately", "about", "psi", "psia", "kpa", "mpa", "pa", "atm", "torr",
    "mmhg", "mm", "hg", "bar", "mbar", "cm/s", "mm/s", "m/s", "cm/sec", "mm/sec", "m/sec", "cm", "m", "s", "sec",
];

fn link(between: &str) -> Link {
    let mut joiners: Vec<String> = vec![];
    for raw in between.split_whitespace() {
        let word = raw.trim_matches(|c: char| matches!(c, '%' | '~' | '(' | ')' | '.'));
        if word.is_empty() || BETWEEN.contains(&word) {
            continue;
        }
        joiners.push(word.to_string());
    }
    match joiners.join(" ").as_str() {
        "-" | "–" | "—" | "to" | "through" => Link::Range,
        "," | "and" | "or" | ", and" | ", or" | "&" | ";" | "/" => Link::List,
        _ => Link::Apart,
    }
}

/// Groups the kept numbers: neighbours joined as a range become one range, the rest stand alone.
fn ranges(kept: &[(usize, f64)], nums: &[Num]) -> Vec<Range> {
    let mut out: Vec<Range> = vec![];
    let mut k = 0;
    while k < kept.len() {
        let (mut lo, mut hi) = (kept[k].1, kept[k].1);
        while k + 1 < kept.len() && kept[k + 1].0 == kept[k].0 + 1 && nums[kept[k].0].link == Link::Range {
            k += 1;
            lo = lo.min(kept[k].1);
            hi = hi.max(kept[k].1);
        }
        out.push(Range::of(lo, hi));
        k += 1;
    }
    out
}

/// One answer or none. Several statements of the same value ("1 atm (101.3 kPa)") agree and
/// count once; several different values are separate conditions, which one row cannot hold.
fn only(found: Vec<Range>) -> Option<Range> {
    let first = *found.first()?;
    let close = |a: f64, b: f64| (a - b).abs() <= 0.015 * a.abs().max(b.abs()).max(1e-9);
    found.iter().all(|r| close(r.min, first.min) && close(r.max, first.max)).then_some(first)
}

fn round(v: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (v * m).round() / m
}

#[derive(Clone, Copy, PartialEq)]
enum O2Unit {
    Percent,
    Fraction,
    /// A partial pressure: kPa per unit.
    Pressure(f64),
}

fn oxygen_unit(after: &str) -> Option<O2Unit> {
    let a = after.trim_start();
    let percent = ["%", "percent", "mol %", "mol%", "mol. %", "mole %", "mole percent", "vol %", "vol%", "vol. %", "volume percent", "volume %"];
    if percent.iter().any(|p| a.starts_with(p)) {
        return Some(O2Unit::Percent);
    }
    let fraction = ["mole fraction", "mol fraction", "mol. fraction", "volume fraction", "molar fraction"];
    // "0.21 O2 mole fraction": the gas name may sit between the number and the unit.
    let lead: String = a.chars().take(24).collect();
    if fraction.iter().any(|p| lead.contains(p)) {
        return Some(O2Unit::Fraction);
    }
    pressure_unit(after).map(O2Unit::Pressure)
}

/// The first word after the number and its unit: which gas the number is a share of, when the text says.
fn gas_after(after: &str) -> Option<String> {
    let unit_words = ["percent", "mol", "mole", "vol", "volume", "by", "fraction", "molar", "kpa", "psi", "psia", "atm", "torr", "mmhg", "mm", "hg", "bar", "mbar", "pa", "mpa", "partial", "pressure", "of"];
    let joiners = ["and", "or", "to", "through"];
    after
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty() && !unit_words.contains(w))
        .filter(|w| !joiners.contains(w))
        .map(|w| w.to_string())
}

/// Percent oxygen stated by this text, or None. `total_kpa` is the same row's pressure: it
/// is used only to turn an oxygen partial pressure into a percentage.
pub fn oxygen_pct(text: &str, total_kpa: Option<Range>) -> Option<Range> {
    if states_a_bound(text) {
        return None;
    }
    let nums = numbers(text);
    let n = nums.len();
    let mut unit: Vec<Option<O2Unit>> = nums.iter().map(|x| oxygen_unit(&x.after)).collect();
    let gas: Vec<Option<String>> = nums.iter().map(|x| gas_after(&x.after)).collect();
    let mut oxygen: Vec<bool> = gas.iter().map(|g| matches!(g.as_deref(), Some("o2") | Some("oxygen"))).collect();
    // "18-21% O2": the unit and the gas written after the last number of a range cover the whole range.
    for k in (0..n.saturating_sub(1)).rev() {
        if nums[k].link != Link::Apart && unit[k].is_none() {
            unit[k] = unit[k + 1];
        }
        // "21% and 30% O2": a number with no gas of its own takes the gas named after the next one.
        if oxygen[k + 1] && (nums[k].link == Link::Range || (nums[k].link == Link::List && gas[k].is_none())) {
            oxygen[k] = true;
        }
    }
    // A number with no unit makes the whole text unreadable: it could be anything.
    if n == 0 || unit.iter().any(|u| u.is_none()) {
        return None;
    }
    let other_gas = ["n2", "nitrogen", "he", "helium", "co2", "ar", "argon", "n", "fuel", "diluent", "inert", "methane", "ch4", "h2", "hydrogen", "co", "total", "chamber"];
    let any_oxygen = oxygen.iter().any(|x| *x);
    let (mut shares, mut partials): (Vec<(usize, f64)>, Vec<(usize, f64)>) = (vec![], vec![]);
    for k in 0..n {
        // When the text says which numbers are oxygen, only those count ("21% O2 and 79% N2").
        if any_oxygen && !oxygen[k] {
            continue;
        }
        if !any_oxygen && gas[k].as_deref().is_some_and(|g| other_gas.contains(&g)) {
            continue;
        }
        match unit[k]? {
            O2Unit::Percent => shares.push((k, nums[k].value)),
            O2Unit::Fraction if nums[k].value <= 1.0 => shares.push((k, round(nums[k].value * 100.0, 4))),
            O2Unit::Fraction => return None,
            O2Unit::Pressure(per) => partials.push((k, nums[k].value * per)),
        }
    }
    let found = if !shares.is_empty() {
        only(ranges(&shares, &nums))?
    } else {
        // A partial pressure is a percentage only against one stated total pressure.
        let part = only(ranges(&partials, &nums))?;
        let total = total_kpa.filter(|t| t.is_single() && t.min > 0.0)?;
        Range::of(round(part.min / total.min * 100.0, 2), round(part.max / total.min * 100.0, 2))
    };
    (found.min > 0.0 && found.max <= 100.0).then_some(found)
}

/// kPa per unit, from the word that follows the number. Gauge pressure ("psig") is not absolute and is refused.
fn pressure_unit(after: &str) -> Option<f64> {
    let a = after.trim_start();
    let word: String = a.chars().take_while(|c| c.is_alphabetic()).collect();
    match word.as_str() {
        "kpa" => Some(1.0),
        "pa" => Some(0.001),
        "mpa" => Some(1000.0),
        "bar" => Some(100.0),
        "mbar" => Some(0.1),
        "psi" | "psia" => Some(6.894_757),
        "atm" | "atmosphere" | "atmospheres" | "atmospheric" => Some(101.325),
        "torr" | "mmhg" => Some(0.133_322_4),
        "mm" if a[word.len()..].trim_start().starts_with("hg") => Some(0.133_322_4),
        _ => None,
    }
}

/// Absolute pressure in kPa stated by this text, or None.
pub fn pressure_kpa(text: &str) -> Option<Range> {
    if states_a_bound(text) {
        return None;
    }
    let nums = numbers(text);
    let n = nums.len();
    let mut unit: Vec<Option<f64>> = nums.iter().map(|x| pressure_unit(&x.after)).collect();
    for k in (0..n.saturating_sub(1)).rev() {
        if nums[k].link != Link::Apart && unit[k].is_none() {
            unit[k] = unit[k + 1];
        }
    }
    if n == 0 || unit.iter().any(|u| u.is_none()) {
        return None;
    }
    let kept: Vec<(usize, f64)> = (0..n).map(|k| (k, round(nums[k].value * unit[k].unwrap_or(0.0), 2))).collect();
    let found = only(ranges(&kept, &nums))?;
    // Zero is only meaningful as the low end of a range ("0 to 3 atm").
    (found.max > 0.0 && found.max <= 200_000.0).then_some(found)
}

/// cm/s per unit, from what follows the number: a length (cm, mm, m) over a second.
fn airflow_unit(after: &str) -> Option<f64> {
    let a = after.trim_start();
    let (per, rest) = if let Some(r) = a.strip_prefix("cm") {
        (1.0, r)
    } else if let Some(r) = a.strip_prefix("mm") {
        (0.1, r)
    } else if let Some(r) = a.strip_prefix('m') {
        (100.0, r)
    } else {
        return None;
    };
    let r = rest.trim_start_matches([' ', '·', '.']);
    let per_second = ["/s", "/ s", "/sec", "/ sec", "s-1", "s^-1", "s−1", "sec-1", "per s"];
    per_second.iter().any(|p| r.starts_with(p)).then_some(per)
}

/// Flow speed in cm/s stated by this text, or None. "Quiescent" or "still air" state no number.
pub fn airflow_cm_s(text: &str) -> Option<Range> {
    if states_a_bound(text) {
        return None;
    }
    let nums = numbers(text);
    let n = nums.len();
    let mut unit: Vec<Option<f64>> = nums.iter().map(|x| airflow_unit(&x.after)).collect();
    for k in (0..n.saturating_sub(1)).rev() {
        if nums[k].link != Link::Apart && unit[k].is_none() {
            unit[k] = unit[k + 1];
        }
    }
    if n == 0 || unit.iter().any(|u| u.is_none()) {
        return None;
    }
    let kept: Vec<(usize, f64)> = (0..n).map(|k| (k, round(nums[k].value * unit[k].unwrap_or(0.0), 3))).collect();
    let found = only(ranges(&kept, &nums))?;
    (found.max <= 100_000.0).then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(v: f64) -> Option<Range> {
        Some(Range { min: v, max: v })
    }
    fn range(a: f64, b: f64) -> Option<Range> {
        Some(Range { min: a, max: b })
    }
    fn o2(text: &str) -> Option<Range> {
        oxygen_pct(text, None)
    }

    #[test]
    fn oxygen_single_values() {
        assert_eq!(o2("30%"), one(30.0));
        assert_eq!(o2("17% O2"), one(17.0));
        assert_eq!(o2("13.6%"), one(13.6));
        assert_eq!(o2("21 vol%"), one(21.0));
        assert_eq!(o2("21 percent (ambient)"), one(21.0));
        assert_eq!(o2("22.0 mol %"), one(22.0));
        assert_eq!(o2("Air (~21% O2)"), one(21.0));
        assert_eq!(o2("Air (approx 21% O2)"), one(21.0));
    }

    #[test]
    fn oxygen_ranges() {
        assert_eq!(o2("21-40% O2"), range(21.0, 40.0));
        assert_eq!(o2("17-83%"), range(17.0, 83.0));
        assert_eq!(o2("14% to 18% by volume"), range(14.0, 18.0));
        assert_eq!(o2("21% to 22% O2"), range(21.0, 22.0));
        assert_eq!(o2("~21% to ~14%"), range(14.0, 21.0));
    }

    #[test]
    fn oxygen_mole_fractions_need_the_words() {
        assert_eq!(o2("0.21 mole fraction"), one(21.0));
        assert_eq!(o2("0.10 to 0.50 mole fraction"), range(10.0, 50.0));
        assert_eq!(o2("near 0.21 O2 mole fraction"), one(21.0));
        // A bare decimal does not say what it is.
        assert_eq!(o2("0.17"), None);
        assert_eq!(o2("0.30"), None);
        assert_eq!(o2("21"), None);
    }

    #[test]
    fn oxygen_ignores_other_gases() {
        assert_eq!(o2("21% O2 and 79% N2"), one(21.0));
        assert_eq!(o2("n-butane/oxygen premixture (50% n-C4H10-50% O2)"), one(50.0));
        assert_eq!(o2("30% O2 in 70% He"), one(30.0));
        assert_eq!(o2("79% N2"), None);
    }

    #[test]
    fn oxygen_several_conditions_or_bounds_stay_null() {
        assert_eq!(o2("15%, 18% and 21%"), None);
        assert_eq!(o2("21% and 30% O2"), None);
        assert_eq!(o2("Varied (tests conducted at levels including 16.1% and lower)"), None);
        assert_eq!(o2("below 16%"), None);
        assert_eq!(o2("up to 40% O2"), None);
        assert_eq!(o2("> 21%"), None);
    }

    #[test]
    fn oxygen_garbage_stays_null() {
        for text in ["", "air", "elevated", "varies", "Varied to determine flammability limits", "air and a mixture of oxygen and helium", "O2", "150%", "0%", "about 21 or so", "21% at 3"] {
            assert_eq!(o2(text), None, "{text:?} should not parse");
        }
    }

    #[test]
    fn oxygen_partial_pressure_needs_the_total() {
        let total = pressure_kpa("101.3 kPa");
        // Not a percentage on its own.
        assert_eq!(oxygen_pct("21.3 kPa O2", None), None);
        assert_eq!(oxygen_pct("3.1 psia oxygen partial pressure", None), None);
        // With one total pressure in the same row it is.
        assert_eq!(oxygen_pct("21.3 kPa O2", total), one(21.03));
        assert_eq!(oxygen_pct("160 mmHg O2", total), one(21.06));
        // A range of total pressure cannot be paired with it.
        assert_eq!(oxygen_pct("21.3 kPa O2", pressure_kpa("50 to 100 kPa")), None);
        // A stated percentage wins and needs no pressure.
        assert_eq!(oxygen_pct("30% O2", total), one(30.0));
    }

    #[test]
    fn pressure_units_convert_to_kpa() {
        assert_eq!(pressure_kpa("100.0 kPa"), one(100.0));
        assert_eq!(pressure_kpa("54.1 kPa"), one(54.1));
        assert_eq!(pressure_kpa("1 atm"), one(101.33));
        assert_eq!(pressure_kpa("0.70 atm"), one(70.93));
        assert_eq!(pressure_kpa("14.7 psia"), one(101.35));
        assert_eq!(pressure_kpa("10.2 psi"), one(70.33));
        assert_eq!(pressure_kpa("1 bar"), one(100.0));
        assert_eq!(pressure_kpa("532.2 mmHg"), one(70.95));
        assert_eq!(pressure_kpa("760 torr"), one(101.33));
        assert_eq!(pressure_kpa("760 mm Hg"), one(101.33));
        assert_eq!(pressure_kpa("0.1 MPa"), one(100.0));
    }

    #[test]
    fn pressure_ranges_and_restatements() {
        assert_eq!(pressure_kpa("0.5 to 1.0 atm"), range(50.66, 101.33));
        assert_eq!(pressure_kpa("35-130 kPa"), range(35.0, 130.0));
        assert_eq!(pressure_kpa("100 to 40 kPa"), range(40.0, 100.0));
        assert_eq!(pressure_kpa("0 to 3 atm"), range(0.0, 303.98));
        // The same pressure written twice is one value.
        assert_eq!(pressure_kpa("1 atm (101.3 kPa)"), one(101.33));
        assert_eq!(pressure_kpa("14.7 psia (101 kPa)"), one(101.35));
    }

    #[test]
    fn pressure_several_conditions_or_garbage_stay_null() {
        for text in [
            "",
            "near ambient",
            "atmospheric",
            "normal atmospheric pressure",
            "Atmospheric pressure (implied by LOI context)",
            "Nominally 100-liter combustion chamber (pressure not explicitly stated as a variable parameter in text)",
            "14.7",
            "5 psig",
            "0 kPa",
            "high",
            "0.7 and 1 atmospheric pressures",
            "1 atm and 3 atm",
            "up to 10 atm",
            "below 1 atm",
        ] {
            assert_eq!(pressure_kpa(text), None, "{text:?} should not parse");
        }
    }

    #[test]
    fn airflow_units_convert_to_cm_per_second() {
        assert_eq!(airflow_cm_s("10 cm/s"), one(10.0));
        assert_eq!(airflow_cm_s("30 cm/s concurrent"), one(30.0));
        assert_eq!(airflow_cm_s("50 mm/s"), one(5.0));
        assert_eq!(airflow_cm_s("0.2 m/s"), one(20.0));
        assert_eq!(airflow_cm_s("20 cm/sec"), one(20.0));
        assert_eq!(airflow_cm_s("5 cm s-1"), one(5.0));
        assert_eq!(airflow_cm_s("0 cm/s"), one(0.0));
        assert_eq!(airflow_cm_s("5-20 cm/s"), range(5.0, 20.0));
        assert_eq!(airflow_cm_s("0 to 30 cm/s"), range(0.0, 30.0));
        assert_eq!(airflow_cm_s("50 to 200 mm/s"), range(5.0, 20.0));
    }

    #[test]
    fn airflow_several_conditions_or_garbage_stay_null() {
        for text in ["", "quiescent", "still air", "low-speed forced flow", "20", "10 cm", "5, 10 and 20 cm/s", "up to 30 cm/s", "< 1 cm/s", "2 L/min", "varied"] {
            assert_eq!(airflow_cm_s(text), None, "{text:?} should not parse");
        }
    }

    #[test]
    fn a_whole_row() {
        let n = parse_condition(Some("30% O2"), Some("10.2 psia"), Some("20 cm/s"));
        assert_eq!(n.oxygen_pct, one(30.0));
        assert_eq!(n.pressure_kpa, one(70.33));
        assert_eq!(n.airflow_cm_s, one(20.0));
        let n = parse_condition(Some("air"), None, Some("quiescent"));
        assert_eq!(n, Numbers::default());
        let n = parse_condition(Some("21.3 kPa O2"), Some("1 atm"), None);
        assert_eq!(n.oxygen_pct, one(21.02));
        assert!(Range { min: 1.0, max: 1.0 }.is_single());
    }
}
