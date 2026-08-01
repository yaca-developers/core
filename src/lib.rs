mod logging;

pub fn add(left: u64, right: u64) -> u64 {
    logging::info!("adding {} and {}", left, right);
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn it_doesnot_work() {
        let result = add(2, 2);
        logging::info!("hi");
        assert_ne!(result, 4);
    }
}
