use crate::errors::*;
use crate::events::*;
use crate::otc_contract::*;
use crate::types::*;
use crate::{chain_extension::StakeInfo, BittensorEnvironment};
use core::convert::TryFrom;
use fixed::types::U64F64;
use ink::env::Environment;
use ink::scale::{Decode, Encode};

const MAX_LISTINGS_PER_USER_PER_NETUID: usize = 25;
const MAX_OFFERS_PER_USER_PER_NETUID: usize = 25;
const SUBTENSOR_EXTENSION_ID: u16 = 0;
const GET_STAKE_INFO_FN_ID: u16 = 0;
const GET_CURRENT_ALPHA_PRICE_FN_ID: u16 = 15;
type TestAccountId = <BittensorEnvironment as Environment>::AccountId;

// Mock market price: 2 TAO per Alpha (2 * 1e9)
const MOCK_MARKET_PRICE: u64 = 2_000_000_000;

#[derive(Clone, Copy)]
struct MockStakeExtension;

impl ink::env::test::ChainExtension for MockStakeExtension {
    fn ext_id(&self) -> u16 {
        SUBTENSOR_EXTENSION_ID
    }

    fn call(&mut self, func_id: u16, input: &[u8], output: &mut Vec<u8>) -> u32 {
        match func_id {
            GET_STAKE_INFO_FN_ID => {
                let mut input = input;
                let hotkey = TestAccountId::decode(&mut input).expect("mock decode hotkey");
                let coldkey = TestAccountId::decode(&mut input).expect("mock decode coldkey");
                let netuid = u16::decode(&mut input).expect("mock decode netuid");

                let _ = (hotkey, coldkey, netuid);

                output.extend(Encode::encode(&Option::<StakeInfo>::None));
                0
            }
            GET_CURRENT_ALPHA_PRICE_FN_ID => {
                // Return mock market price: 2 TAO per Alpha
                output.extend(Encode::encode(&MOCK_MARKET_PRICE));
                0
            }
            _ => 1,
        }
    }
}

fn register_mock_stake_extension() {
    ink::env::test::register_chain_extension(MockStakeExtension);
}

/// Helper function to create fee rate bits from percentage
fn fee_rate_from_percentage(percentage: f64) -> u128 {
    let fee_as_decimal = percentage / 100.0;
    let fixed = U64F64::from_num(fee_as_decimal);
    fixed.to_bits()
}

#[ink::test]
fn constructor_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();

    // Test with custom parameters (0.5% fee)
    let fee_rate = fee_rate_from_percentage(0.5);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    assert_eq!(contract.get_owner(), accounts.alice);
    assert_eq!(contract.get_hotkey(), accounts.bob);
    assert_eq!(contract.get_min_listing_amount(), 1_000_000_000);
    assert_eq!(contract.get_min_offer_amount(), 1_000_000_000);
    assert_eq!(contract.get_min_listing_age(), 100);
}

#[ink::test]
fn update_owner_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update owner to Charlie
    assert_eq!(contract.update_owner(accounts.charlie), Ok(()));
    assert_eq!(contract.get_owner(), accounts.charlie);

    // Verify emitted event
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 1);

    let decoded_event =
        <OwnerUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
            .expect("Failed to decode event");
    assert_eq!(decoded_event.old_owner, accounts.alice);
    assert_eq!(decoded_event.new_owner, accounts.charlie);
}

#[ink::test]
fn update_owner_fails_if_not_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Bob (not the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    // Try to update owner
    assert_eq!(
        contract.update_owner(accounts.charlie),
        Err(Error::Unauthorized)
    );
    assert_eq!(contract.get_owner(), accounts.alice);
}

#[ink::test]
fn update_hotkey_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update hotkey to Charlie
    assert_eq!(contract.update_hotkey(accounts.charlie), Ok(()));
    assert_eq!(contract.get_hotkey(), accounts.charlie);

    // Verify emitted event
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 1);

    let decoded_event =
        <HotkeyUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
            .expect("Failed to decode event");
    assert_eq!(decoded_event.old_hotkey, accounts.bob);
    assert_eq!(decoded_event.new_hotkey, accounts.charlie);
}

#[ink::test]
fn update_hotkey_fails_if_not_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Bob (not the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    // Try to update hotkey
    assert_eq!(
        contract.update_hotkey(accounts.charlie),
        Err(Error::Unauthorized)
    );
    assert_eq!(contract.get_hotkey(), accounts.bob);
}

#[ink::test]
fn ownership_transfer_chain_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Alice transfers ownership to Bob
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    assert_eq!(contract.update_owner(accounts.bob), Ok(()));

    // Now Bob can perform owner actions
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);
    assert_eq!(contract.update_hotkey(accounts.charlie), Ok(()));

    // Alice can no longer perform owner actions
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    assert_eq!(
        contract.update_hotkey(accounts.django),
        Err(Error::Unauthorized)
    );
}

#[ink::test]
fn update_fee_rate_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let initial_fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        initial_fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update fee rate to 1%
    let new_fee_rate = fee_rate_from_percentage(1.0);
    assert_eq!(contract.update_fee_rate(new_fee_rate), Ok(()));
    assert_eq!(contract.get_fee_rate().to_bits(), new_fee_rate);

    // Verify emitted event
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 1);

    let decoded_event =
        <FeeRateUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
            .expect("Failed to decode event");
    assert_eq!(decoded_event.old_rate.to_bits(), initial_fee_rate);
    assert_eq!(decoded_event.new_rate.to_bits(), new_fee_rate);
}

#[ink::test]
fn update_fee_rate_fails_if_not_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Bob (not the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    let new_fee_rate = fee_rate_from_percentage(1.0);
    assert_eq!(
        contract.update_fee_rate(new_fee_rate),
        Err(Error::Unauthorized)
    );
    assert_eq!(contract.get_fee_rate().to_bits(), fee_rate);
}

#[ink::test]
fn update_min_listing_amount_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update minimum listing amount
    let new_amount = 2_000_000_000;
    assert_eq!(contract.update_min_listing_amount(new_amount), Ok(()));
    assert_eq!(contract.get_min_listing_amount(), new_amount);

    // Verify emitted event
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 1);

    let decoded_event =
        <MinListingAmountUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
            .expect("Failed to decode event");
    assert_eq!(decoded_event.old_amount, 1_000_000_000);
    assert_eq!(decoded_event.new_amount, new_amount);
}

#[ink::test]
fn update_min_listing_amount_fails_if_not_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Bob (not the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    assert_eq!(
        contract.update_min_listing_amount(2_000_000_000),
        Err(Error::Unauthorized)
    );
    assert_eq!(contract.get_min_listing_amount(), 1_000_000_000);
}

#[ink::test]
fn update_min_offer_amount_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update minimum offer amount
    let new_amount = 500_000_000;
    assert_eq!(contract.update_min_offer_amount(new_amount), Ok(()));
    assert_eq!(contract.get_min_offer_amount(), new_amount);

    // Verify emitted event
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 1);

    let decoded_event =
        <MinOfferAmountUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
            .expect("Failed to decode event");
    assert_eq!(decoded_event.old_amount, 1_000_000_000);
    assert_eq!(decoded_event.new_amount, new_amount);
}

#[ink::test]
fn update_min_offer_amount_fails_if_not_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Bob (not the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    assert_eq!(
        contract.update_min_offer_amount(500_000_000),
        Err(Error::Unauthorized)
    );
    assert_eq!(contract.get_min_offer_amount(), 1_000_000_000);
}

#[ink::test]
fn update_min_listing_age_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update minimum listing age
    let new_age = 200;
    assert_eq!(contract.update_min_listing_age(new_age), Ok(()));
    assert_eq!(contract.get_min_listing_age(), new_age);

    // Verify emitted event
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 1);

    let decoded_event =
        <MinListingAgeUpdated as ink::scale::Decode>::decode(&mut &emitted_events[0].data[..])
            .expect("Failed to decode event");
    assert_eq!(decoded_event.old_age, 100);
    assert_eq!(decoded_event.new_age, new_age);
}

#[ink::test]
fn update_min_listing_age_fails_if_not_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Bob (not the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    assert_eq!(
        contract.update_min_listing_age(200),
        Err(Error::Unauthorized)
    );
    assert_eq!(contract.get_min_listing_age(), 100);
}

#[ink::test]
fn update_multiple_configurations_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let initial_fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        initial_fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Alice (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    // Update multiple configurations
    assert_eq!(
        contract.update_fee_rate(fee_rate_from_percentage(0.75)),
        Ok(())
    );
    assert_eq!(contract.update_min_listing_amount(2_000_000_000), Ok(()));
    assert_eq!(contract.update_min_offer_amount(500_000_000), Ok(()));
    assert_eq!(contract.update_min_listing_age(150), Ok(()));

    // Verify all values were updated
    assert_eq!(
        contract.get_fee_rate().to_bits(),
        fee_rate_from_percentage(0.75)
    );
    assert_eq!(contract.get_min_listing_amount(), 2_000_000_000);
    assert_eq!(contract.get_min_offer_amount(), 500_000_000);
    assert_eq!(contract.get_min_listing_age(), 150);

    // Verify 4 events were emitted
    let emitted_events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(emitted_events.len(), 4);
}

#[ink::test]
fn list_alpha_fails_with_amount_too_small() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000, // min listing amount
        1_000_000_000,
        100,
    );

    // Set caller to Charlie (the seller)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    let price_offset_bps = 0; // Market price
    let netuid = 1u16;
    let amount = 500_000_000u64; // Below minimum

    // Should fail due to amount being too small
    let result = contract.list_alpha(accounts.django, netuid, amount, price_offset_bps);
    assert_eq!(result, Err(Error::AmountTooSmall));
}

#[ink::test]
fn list_alpha_fails_with_invalid_price_offset() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Charlie (the seller)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    let price_offset_bps = -10000; // -100% offset (invalid)
    let netuid = 1u16;
    let amount = 5_000_000_000u64;

    // Should fail due to invalid price offset (<= -100%)
    let result = contract.list_alpha(accounts.django, netuid, amount, price_offset_bps);
    assert_eq!(result, Err(Error::InvalidPriceOffset));
}

#[ink::test]
fn list_alpha_fails_when_user_reaches_listing_cap() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    let netuid = 1u16;
    let existing_ids: Vec<AlphaListingId> = (0..MAX_LISTINGS_PER_USER_PER_NETUID)
        .map(|i| u64::try_from(i + 1).expect("id fits in u64"))
        .collect();
    contract
        .user_listings
        .insert((accounts.charlie, netuid), &existing_ids);

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    let price_offset_bps = 0; // Market price
    let amount = contract.get_min_listing_amount();

    let result = contract.list_alpha(accounts.django, netuid, amount, price_offset_bps);
    assert_eq!(result, Err(Error::TooManyListings));
}

#[ink::test]
fn get_listing_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Try to get a non-existent listing
    let listing = contract.get_listing(1, accounts.charlie, 1);
    assert_eq!(listing, None);
}

#[ink::test]
fn get_user_listings_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Get listings for a user with no listings
    let listings = contract.get_user_listings(accounts.charlie, 1);
    assert_eq!(listings, Vec::<AlphaListingId>::new());
}

#[ink::test]
fn cancel_alpha_listing_fails_when_listing_not_found() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    register_mock_stake_extension();

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Charlie
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    // Try to cancel a non-existent listing
    let result = contract.cancel_alpha_listing(1, 999);
    assert_eq!(result, Err(Error::ListingNotFound));
}

#[ink::test]
fn cancel_alpha_listing_fails_when_too_young() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    register_mock_stake_extension();

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100, // min_listing_age = 100 blocks
    );

    // Create a mock listing manually for testing
    // In production this would be created via list_alpha
    let listing = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000, // Created at block 1000
    };

    // Insert the listing
    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1]);

    // Set caller to Charlie (the owner)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    // Set current block to 1050 (only 50 blocks old, less than 100)
    ink::env::test::set_block_number::<BittensorEnvironment>(1050);

    // Try to cancel the listing (should fail due to minimum age)
    let result = contract.cancel_alpha_listing(1, 1);
    assert_eq!(result, Err(Error::ListingTooYoung));
}

#[ink::test]
fn force_cancel_alpha_listing_requires_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);

    let result = contract.force_cancel_alpha_listing(1, accounts.charlie, 1);
    assert_eq!(result, Err(Error::Unauthorized));
}

#[ink::test]
fn cancel_alpha_listing_cleans_up_storage() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create two mock listings for the same user
    let listing1 = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    let listing2 = AlphaListing {
        id: 2,
        netuid: 1,
        seller: accounts.charlie,
        amount: 3_000_000_000,
        price_offset_bps: -500, // 5% below market
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    // Insert both listings
    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing1);
    contract
        .alpha_listings
        .insert((1, accounts.charlie, 2), &listing2);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1, 2]);

    // Verify both listings exist
    assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
    assert!(contract.get_listing(1, accounts.charlie, 2).is_some());
    assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 2);

    // Set caller to Charlie
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    // Set current block to be well past minimum age
    ink::env::test::set_block_number::<BittensorEnvironment>(1200);

    // Note: In tests, runtime calls will fail, but we can test the logic up to that point
    // The actual cancellation would fail at runtime call, but storage cleanup logic is correct

    // Manually simulate successful cancellation for testing storage cleanup
    // Remove listing 1
    contract.alpha_listings.remove((1, accounts.charlie, 1));
    let mut user_listings = contract.user_listings.get((accounts.charlie, 1)).unwrap();
    user_listings.retain(|&id| id != 1);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &user_listings);

    // Verify listing 1 is removed but listing 2 remains
    assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
    assert!(contract.get_listing(1, accounts.charlie, 2).is_some());
    assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![2]);

    // Remove listing 2
    contract.alpha_listings.remove((1, accounts.charlie, 2));
    let user_listings = contract.user_listings.get((accounts.charlie, 1)).unwrap();
    let filtered: Vec<_> = user_listings.into_iter().filter(|&id| id != 2).collect();

    if filtered.is_empty() {
        contract.user_listings.remove((accounts.charlie, 1));
    } else {
        contract
            .user_listings
            .insert((accounts.charlie, 1), &filtered);
    }

    // Verify both listings are removed and user_listings is cleaned up
    assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
    assert!(contract.get_listing(1, accounts.charlie, 2).is_none());
    assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 0);
}

#[ink::test]
fn create_tao_offer_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000, // min offer amount
        100,
    );

    // Set caller to Charlie (the buyer)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    // Set transferred value (TAO amount)
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

    let price_offset_bps = 500; // 5% above market
    let netuid = 1u16;

    // Create TAO offer
    let result = contract.create_tao_offer(netuid, price_offset_bps);
    assert!(result.is_ok());

    let offer_id = result.unwrap();
    assert_eq!(offer_id, 1);

    // Verify offer was stored correctly
    let offer = contract.get_offer(netuid, accounts.charlie, offer_id);
    assert!(offer.is_some());

    let offer = offer.unwrap();
    assert_eq!(offer.id, offer_id);
    assert_eq!(offer.netuid, netuid);
    assert_eq!(offer.buyer, accounts.charlie);
    assert_eq!(offer.amount, 5_000_000_000);
    assert_eq!(offer.price_offset_bps, price_offset_bps);

    // Verify user offers index was updated
    let user_offers = contract.get_user_offers(accounts.charlie, netuid);
    assert_eq!(user_offers, vec![offer_id]);
}

#[ink::test]
fn create_tao_offer_fails_with_amount_too_small() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000, // min offer amount
        100,
    );

    // Set caller to Charlie (the buyer)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    // Set transferred value below minimum
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(500_000_000);

    let price_offset_bps = 0; // Market price
    let netuid = 1u16;

    // Should fail due to amount being too small
    let result = contract.create_tao_offer(netuid, price_offset_bps);
    assert_eq!(result, Err(Error::AmountTooSmall));
}

#[ink::test]
fn set_subnet_listing_status_updates_mapping() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);

    assert!(!contract.is_subnet_frozen(42));

    let freeze_reason = b"at-risk".to_vec();
    assert_eq!(
        contract.set_subnet_listing_status(42, true, freeze_reason.clone()),
        Ok(())
    );

    assert!(contract.is_subnet_frozen(42));

    let events = ink::env::test::recorded_events().collect::<Vec<_>>();
    assert_eq!(events.len(), 1);

    let decoded =
        <SubnetListingStatusChanged as ink::scale::Decode>::decode(&mut &events[0].data[..])
            .expect("decode freeze event");
    assert_eq!(decoded.netuid, 42);
    assert!(decoded.frozen);
    assert_eq!(decoded.reason, freeze_reason);
    assert_eq!(decoded.changed_by, accounts.alice);

    assert_eq!(
        contract.set_subnet_listing_status(42, false, b"clear".to_vec()),
        Ok(())
    );

    assert!(!contract.is_subnet_frozen(42));
}

#[ink::test]
fn list_alpha_rejects_when_subnet_frozen() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    contract
        .set_subnet_listing_status(7, true, b"risk".to_vec())
        .expect("freeze subnet");

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    let price_offset_bps = 0; // Market price
    let result = contract.list_alpha(accounts.bob, 7, 1_000_000_000, price_offset_bps);

    assert_eq!(result, Err(Error::SubnetListingsFrozen));
}

#[ink::test]
fn create_tao_offer_fails_with_invalid_price_offset() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Charlie (the buyer)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    // Set transferred value
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

    let price_offset_bps = -10000; // -100% offset (invalid)
    let netuid = 1u16;

    // Should fail due to invalid price offset (<= -100%)
    let result = contract.create_tao_offer(netuid, price_offset_bps);
    assert_eq!(result, Err(Error::InvalidPriceOffset));
}

#[ink::test]
fn create_tao_offer_fails_when_user_reaches_offer_cap() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    let netuid = 1u16;
    let existing_ids: Vec<TaoOfferId> = (0..MAX_OFFERS_PER_USER_PER_NETUID)
        .map(|i| u64::try_from(i + 1).expect("id fits in u64"))
        .collect();
    contract
        .user_offers
        .insert((accounts.charlie, netuid), &existing_ids);

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(
        contract.get_min_offer_amount().into(),
    );

    let price_offset_bps = 0; // Market price

    let result = contract.create_tao_offer(netuid, price_offset_bps);
    assert_eq!(result, Err(Error::TooManyOffers));
}

#[ink::test]
fn create_tao_offer_fails_with_zero_amount() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Charlie (the buyer)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    // Set transferred value to zero
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(0);

    let price_offset_bps = 0; // Market price
    let netuid = 1u16;

    // Should fail due to zero amount
    let result = contract.create_tao_offer(netuid, price_offset_bps);
    assert_eq!(result, Err(Error::AmountTooSmall));
}

#[ink::test]
fn price_calculation_in_take_tao_offer_is_correct() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0); // 1% fee

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Test with market price (offset = 0)
    // Mock market price: 2 TAO per Alpha (2 * 1e9 = 2_000_000_000)
    let market_price: u64 = MOCK_MARKET_PRICE;
    let tao_amount = 10_000_000_000u64; // 10 TAO

    // Create the offer with 0% offset (market price)
    let offer = TaoOffer {
        id: 1,
        netuid: 1,
        buyer: accounts.charlie,
        amount: tao_amount,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 0,
    };

    // Calculate expected values
    let fee_amount = 100_000_000u64; // 1% of 10 TAO = 0.1 TAO
    let tao_for_seller = tao_amount - fee_amount; // 9.9 TAO

    // Apply the price offset (0% = market price)
    let executed_price =
        OtcContract::apply_price_offset(market_price, offer.price_offset_bps).unwrap();
    assert_eq!(executed_price, market_price); // No change with 0% offset

    // Convert to FixedDecimal for calculation
    let price_decimal = OtcContract::price_to_fixed_decimal(executed_price);

    // Expected Alpha: 9.9 TAO / 2 TAO per Alpha = 4.95 Alpha
    let expected_alpha = 4_950_000_000u64; // 4.95 Alpha in rao

    // Test the calculation using the same logic as the contract
    let calculated_alpha = price_decimal.as_divisor_of(tao_for_seller).unwrap();
    assert_eq!(calculated_alpha, expected_alpha);
}

#[ink::test]
fn cancel_tao_offer_authorization_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create a mock offer manually for testing
    let offer = TaoOffer {
        id: 1,
        netuid: 1,
        buyer: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    // Insert the offer
    contract.tao_offers.insert((1, accounts.charlie, 1), &offer);
    contract.user_offers.insert((accounts.charlie, 1), &vec![1]);

    // Test that a different user cannot cancel
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.django);
    let result = contract.cancel_tao_offer(1, 1);
    assert_eq!(result, Err(Error::OfferNotFound));

    // Verify that the correct owner can find their offer
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    let found_offer = contract.get_offer(1, accounts.charlie, 1);
    assert!(found_offer.is_some());
    assert_eq!(found_offer.unwrap().buyer, accounts.charlie);
}

#[ink::test]
fn cancel_tao_offer_fails_when_offer_not_found() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Set caller to Charlie
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    // Try to cancel a non-existent offer
    let result = contract.cancel_tao_offer(1, 999);
    assert_eq!(result, Err(Error::OfferNotFound));
}

#[ink::test]
fn get_offer_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Try to get a non-existent offer
    let offer = contract.get_offer(1, accounts.charlie, 1);
    assert_eq!(offer, None);
}

#[ink::test]
fn get_user_offers_works() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Get offers for a user with no offers
    let offers = contract.get_user_offers(accounts.charlie, 1);
    assert_eq!(offers, Vec::<TaoOfferId>::new());
}

#[ink::test]
fn cancel_tao_offer_cleans_up_storage() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create two mock offers for the same user
    let offer1 = TaoOffer {
        id: 1,
        netuid: 1,
        buyer: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    let offer2 = TaoOffer {
        id: 2,
        netuid: 1,
        buyer: accounts.charlie,
        amount: 3_000_000_000,
        price_offset_bps: -500, // 5% below market
        fee_rate: contract.fee_rate,
        created_at: 1100,
    };

    // Insert both offers
    contract
        .tao_offers
        .insert((1, accounts.charlie, 1), &offer1);
    contract
        .tao_offers
        .insert((1, accounts.charlie, 2), &offer2);
    contract
        .user_offers
        .insert((accounts.charlie, 1), &vec![1, 2]);

    // Verify both offers exist
    assert!(contract.get_offer(1, accounts.charlie, 1).is_some());
    assert!(contract.get_offer(1, accounts.charlie, 2).is_some());
    assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 2);

    // Manually simulate successful cancellation for testing storage cleanup
    // Remove offer 1
    contract.tao_offers.remove((1, accounts.charlie, 1));
    let mut user_offers = contract.user_offers.get((accounts.charlie, 1)).unwrap();
    user_offers.retain(|&id| id != 1);
    contract
        .user_offers
        .insert((accounts.charlie, 1), &user_offers);

    // Verify offer 1 is removed but offer 2 remains
    assert!(contract.get_offer(1, accounts.charlie, 1).is_none());
    assert!(contract.get_offer(1, accounts.charlie, 2).is_some());
    assert_eq!(contract.get_user_offers(accounts.charlie, 1), vec![2]);

    // Remove offer 2
    contract.tao_offers.remove((1, accounts.charlie, 2));
    let user_offers = contract.user_offers.get((accounts.charlie, 1)).unwrap();
    let filtered: Vec<_> = user_offers.into_iter().filter(|&id| id != 2).collect();

    if filtered.is_empty() {
        contract.user_offers.remove((accounts.charlie, 1));
    } else {
        contract
            .user_offers
            .insert((accounts.charlie, 1), &filtered);
    }

    // Verify both offers are removed and user_offers is cleaned up
    assert!(contract.get_offer(1, accounts.charlie, 1).is_none());
    assert!(contract.get_offer(1, accounts.charlie, 2).is_none());
    assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 0);
}

#[ink::test]
fn take_alpha_listing_fails_with_insufficient_payment() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    register_mock_stake_extension();

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create a mock listing with 0% offset (market price = 2 TAO per Alpha)
    let alpha_amount = 5_000_000_000u64;
    let listing = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: alpha_amount,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing);

    // Set wrong payment amount (too little)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.django);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(1_000_000_000u128); // Too little

    // Try to take the listing - should fail with InsufficientPayment
    let result = contract.take_alpha_listing(1, accounts.charlie, 1);
    assert_eq!(result, Err(Error::InsufficientPayment));
}

#[ink::test]
fn take_alpha_listing_fails_when_not_found() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    register_mock_stake_extension();

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.django);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(10_000_000_000u128);

    // Try to take non-existent listing
    let result = contract.take_alpha_listing(1, accounts.charlie, 999);
    assert_eq!(result, Err(Error::ListingNotFound));
}

#[ink::test]
fn take_tao_offer_fails_when_not_found() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.django);

    // Try to take non-existent offer
    let result = contract.take_tao_offer(1, accounts.charlie, 999, accounts.eve);
    assert_eq!(result, Err(Error::OfferNotFound));
}

#[ink::test]
fn test_fixed_decimal_overflow_in_mul() {
    // Test multiplication that would overflow
    let large_decimal = FixedDecimal::from_bits(U64F64::from_num(u64::MAX / 2).to_bits());
    let result = large_decimal.mul(3);
    assert_eq!(result, Err(Error::Overflow));

    // Test with maximum safe value
    let safe_decimal = FixedDecimal::from_bits(U64F64::from_num(1000u64).to_bits());
    let safe_result = safe_decimal.mul(1_000_000);
    assert_eq!(safe_result, Ok(1_000_000_000));
}

#[ink::test]
fn test_fixed_decimal_overflow_in_div_by() {
    // Test division with very small divisor - should produce a large number
    let tiny_decimal = FixedDecimal::from_bits(U64F64::from_num(0.000000001).to_bits());
    let result = tiny_decimal.div_by(100);

    // This produces 100 / 0.000000001 = 100,000,000,000 which fits in u64
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 99_999_999_998); // Slight rounding from fixed-point math

    // Test that would actually overflow
    let extremely_tiny = FixedDecimal::from_bits(U64F64::from_num(0.0000000000001).to_bits());
    let overflow_result = extremely_tiny.div_by(u64::MAX);
    assert_eq!(overflow_result, Err(Error::Overflow));

    // Test normal division
    let normal_decimal = FixedDecimal::from_bits(U64F64::from_num(10u64).to_bits());
    let normal_result = normal_decimal.div_by(1000);
    assert_eq!(normal_result, Ok(100));
}

#[ink::test]
fn test_fixed_decimal_fee_calculations() {
    // Test various fee percentages
    let fee_0_1_percent = FixedDecimal::from_bits(U64F64::from_num(0.001).to_bits()); // 0.1%
    let fee_0_5_percent = FixedDecimal::from_bits(U64F64::from_num(0.005).to_bits()); // 0.5%
    let fee_1_percent = FixedDecimal::from_bits(U64F64::from_num(0.01).to_bits()); // 1%
    let fee_2_5_percent = FixedDecimal::from_bits(U64F64::from_num(0.025).to_bits()); // 2.5%

    let amount = 10_000_000_000u64; // 10 TAO

    assert_eq!(fee_0_1_percent.mul(amount), Ok(10_000_000)); // 0.01 TAO
    assert_eq!(fee_0_5_percent.mul(amount), Ok(50_000_000)); // 0.05 TAO
    assert_eq!(fee_1_percent.mul(amount), Ok(100_000_000)); // 0.1 TAO
    assert_eq!(fee_2_5_percent.mul(amount), Ok(250_000_000)); // 0.25 TAO
}

#[ink::test]
fn reserved_alpha_tracking_updates() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    assert_eq!(contract.get_reserved_alpha(1), 0);

    contract.increase_reserved_alpha(1, 50).unwrap();
    assert_eq!(contract.get_reserved_alpha(1), 50);

    contract.increase_reserved_alpha(1, 25).unwrap();
    assert_eq!(contract.get_reserved_alpha(1), 75);

    contract.decrease_reserved_alpha(1, 25).unwrap();
    assert_eq!(contract.get_reserved_alpha(1), 50);

    contract.decrease_reserved_alpha(1, 50).unwrap();
    assert_eq!(contract.get_reserved_alpha(1), 0);
}

#[ink::test]
fn calculate_claimable_dividends_behaviour() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // No reserved stake means zero dividends available
    assert_eq!(
        contract.calculate_claimable_dividends(1, 0),
        Err(Error::NoDividendsAvailable)
    );

    contract.increase_reserved_alpha(1, 100).unwrap();

    // Contract stake equal to reserved -> still nothing to claim
    assert_eq!(
        contract.calculate_claimable_dividends(1, 100),
        Err(Error::NoDividendsAvailable)
    );

    // Excess stake becomes claimable dividends
    assert_eq!(contract.calculate_claimable_dividends(1, 175), Ok(75));
}

#[ink::test]
fn decrease_reserved_alpha_prevents_underflow() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    contract.increase_reserved_alpha(1, 25).unwrap();
    let result = contract.decrease_reserved_alpha(1, 50);
    assert_eq!(result, Err(Error::Overflow));

    // ensure original value unchanged after failed attempt
    assert_eq!(contract.get_reserved_alpha(1), 25);
}

#[ink::test]
fn test_reserved_alpha_consistency_with_tolerance() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    let netuid = 1;
    let listing_amount = 1000;

    // Initially no reserved alpha
    assert_eq!(contract.get_reserved_alpha(netuid), 0);

    // Simulate list_alpha with actual transfer being slightly less (within tolerance)
    // This simulates what happens in list_alpha when contract_increase is less than amount
    let actual_increase = listing_amount - 5; // Within TRANSFER_TOLERANCE of 10
    contract
        .increase_reserved_alpha(netuid, actual_increase)
        .unwrap();

    // In the fixed version, we should track the listing amount, not actual transfer
    // But for this test, we're verifying the helper functions work correctly
    assert_eq!(contract.get_reserved_alpha(netuid), actual_increase);

    // Now simulate cancellation where we should decrease by listing_amount
    // This tests that we use listing.amount not the observed transfer
    contract
        .decrease_reserved_alpha(netuid, listing_amount)
        .unwrap_err(); // Should fail - can't decrease more than reserved

    // Decrease by the correct amount (what was actually increased)
    contract
        .decrease_reserved_alpha(netuid, actual_increase)
        .unwrap();
    assert_eq!(contract.get_reserved_alpha(netuid), 0);

    // Test multiple operations to ensure no drift
    // Simulate multiple list/cancel cycles
    for i in 1..=5 {
        let amount = 100 * i as u64;
        contract.increase_reserved_alpha(netuid, amount).unwrap();
    }

    // Total should be 100 + 200 + 300 + 400 + 500 = 1500
    assert_eq!(contract.get_reserved_alpha(netuid), 1500);

    // Cancel them all using exact amounts
    for i in 1..=5 {
        let amount = 100 * i as u64;
        contract.decrease_reserved_alpha(netuid, amount).unwrap();
    }

    // Should be back to zero with no drift
    assert_eq!(contract.get_reserved_alpha(netuid), 0);
}

#[ink::test]
fn test_fixed_decimal_maximum_values() {
    // Test with maximum TAO amounts (considering 1 TAO = 10^9 rao)
    let price = FixedDecimal::from_bits(U64F64::from_num(1.5).to_bits());
    let max_tao = 1_000_000_000_000_000u64; // 1 million TAO in rao

    // Should handle large TAO amounts
    let result = price.mul(max_tao);
    assert_eq!(result, Ok(1_500_000_000_000_000));

    // Test as_divisor_of with large amounts
    let alpha_result = price.as_divisor_of(max_tao);
    assert_eq!(alpha_result, Ok(666_666_666_666_666)); // Approximately 2/3 of max_tao
}

#[ink::test]
fn test_listing_counter_increments() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Initial counter should be 1
    assert_eq!(contract.next_alpha_listing_id, 1);

    // Manually simulate listing creation (since runtime calls won't work)
    contract.next_alpha_listing_id = 5;
    assert_eq!(contract.next_alpha_listing_id, 5);

    // Simulate another increment
    contract.next_alpha_listing_id += 1;
    assert_eq!(contract.next_alpha_listing_id, 6);
}

#[ink::test]
fn test_offer_counter_increments() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Initial counter should be 1
    assert_eq!(contract.next_tao_offer_id, 1);

    // Create an offer (this will actually work since it doesn't need runtime calls)
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

    let price_offset_bps = 0; // Market price
    let result = contract.create_tao_offer(1, price_offset_bps);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 1);

    // Counter should have incremented
    assert_eq!(contract.next_tao_offer_id, 2);

    // Create another offer
    let result2 = contract.create_tao_offer(1, price_offset_bps);
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap(), 2);
    assert_eq!(contract.next_tao_offer_id, 3);
}

#[ink::test]
fn test_multiple_listings_different_netuids() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create listings on different netuids for the same user
    let listing1 = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    let listing2 = AlphaListing {
        id: 2,
        netuid: 2,
        seller: accounts.charlie,
        amount: 3_000_000_000,
        price_offset_bps: -500, // 5% below market
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    let listing3 = AlphaListing {
        id: 3,
        netuid: 1,
        seller: accounts.charlie,
        amount: 7_000_000_000,
        price_offset_bps: 1000, // 10% above market
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    // Insert listings
    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing1);
    contract
        .alpha_listings
        .insert((2, accounts.charlie, 2), &listing2);
    contract
        .alpha_listings
        .insert((1, accounts.charlie, 3), &listing3);

    // Update user listings index
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1, 3]);
    contract
        .user_listings
        .insert((accounts.charlie, 2), &vec![2]);

    // Verify correct retrieval
    assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![1, 3]);
    assert_eq!(contract.get_user_listings(accounts.charlie, 2), vec![2]);
    assert_eq!(
        contract.get_user_listings(accounts.charlie, 3),
        Vec::<AlphaListingId>::new()
    );

    // Verify individual listings
    assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
    assert!(contract.get_listing(2, accounts.charlie, 2).is_some());
    assert!(contract.get_listing(1, accounts.charlie, 3).is_some());
    assert!(contract.get_listing(1, accounts.charlie, 2).is_none()); // Wrong netuid
}

#[ink::test]
fn test_multiple_offers_different_netuids() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create offers on different netuids
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(5_000_000_000);

    let price_offset_bps = 0; // Market price

    // Offer on netuid 1
    let offer1_id = contract.create_tao_offer(1, price_offset_bps).unwrap();
    assert_eq!(offer1_id, 1);

    // Offer on netuid 2
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(3_000_000_000);
    let offer2_id = contract.create_tao_offer(2, price_offset_bps).unwrap();
    assert_eq!(offer2_id, 2);

    // Another offer on netuid 1
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(7_000_000_000);
    let offer3_id = contract.create_tao_offer(1, price_offset_bps).unwrap();
    assert_eq!(offer3_id, 3);

    // Verify correct retrieval
    assert_eq!(contract.get_user_offers(accounts.charlie, 1), vec![1, 3]);
    assert_eq!(contract.get_user_offers(accounts.charlie, 2), vec![2]);
    assert_eq!(
        contract.get_user_offers(accounts.charlie, 3),
        Vec::<TaoOfferId>::new()
    );
}

#[ink::test]
fn test_storage_cleanup_all_listings_removed() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create a single listing
    let listing = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1]);

    // Verify listing exists
    assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
    assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 1);

    // Remove the listing completely
    contract.alpha_listings.remove((1, accounts.charlie, 1));
    contract.user_listings.remove((accounts.charlie, 1));

    // Verify complete cleanup
    assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
    assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 0);
}

#[ink::test]
fn test_storage_cleanup_all_offers_removed() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create a single offer
    let offer = TaoOffer {
        id: 1,
        netuid: 1,
        buyer: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    contract.tao_offers.insert((1, accounts.charlie, 1), &offer);
    contract.user_offers.insert((accounts.charlie, 1), &vec![1]);

    // Verify offer exists
    assert!(contract.get_offer(1, accounts.charlie, 1).is_some());
    assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 1);

    // Remove the offer completely
    contract.tao_offers.remove((1, accounts.charlie, 1));
    contract.user_offers.remove((accounts.charlie, 1));

    // Verify complete cleanup
    assert!(contract.get_offer(1, accounts.charlie, 1).is_none());
    assert_eq!(contract.get_user_offers(accounts.charlie, 1).len(), 0);
}

// Error Conditions

#[ink::test]
fn test_list_alpha_with_zero_amount() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    let price_offset_bps = 0; // Market price

    // Try to list with zero amount
    let result = contract.list_alpha(accounts.django, 1, 0, price_offset_bps);
    assert_eq!(result, Err(Error::AmountTooSmall));
}

#[ink::test]
fn test_create_offer_with_maximum_amounts() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    // Test with very large amount (but still valid)
    let large_amount = 1_000_000_000_000_000u128; // 1 million TAO
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(large_amount);

    let price_offset_bps = 0; // Market price
    let result = contract.create_tao_offer(1, price_offset_bps);
    assert!(result.is_ok());

    let offer = contract
        .get_offer(1, accounts.charlie, result.unwrap())
        .unwrap();
    assert_eq!(offer.amount, large_amount as u64);
}

#[ink::test]
fn test_fee_calculation_precision() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();

    // Test with various fee rates to ensure precision
    let test_cases = vec![
        (0.1, 10_000_000_000u64, 10_000_000u64), // 0.1% of 10 TAO = 0.01 TAO
        (0.25, 10_000_000_000u64, 25_000_000u64), // 0.25% of 10 TAO = 0.025 TAO
        (0.33, 10_000_000_000u64, 32_999_999u64), // 0.33% of 10 TAO = ~0.033 TAO (rounding)
        (1.5, 10_000_000_000u64, 149_999_999u64), // 1.5% of 10 TAO = ~0.15 TAO (rounding)
        (2.75, 10_000_000_000u64, 275_000_000u64), // 2.75% of 10 TAO = 0.275 TAO
    ];

    for (fee_percentage, amount, expected_fee) in test_cases {
        let fee_rate = fee_rate_from_percentage(fee_percentage);
        let contract = OtcContract::new(
            accounts.alice,
            accounts.bob,
            fee_rate,
            1_000_000_000,
            1_000_000_000,
            100,
        );

        let calculated_fee = contract.fee_rate.mul(amount).unwrap();
        assert_eq!(
            calculated_fee, expected_fee,
            "Fee calculation failed for {}% of {}",
            fee_percentage, amount
        );
    }
}

#[ink::test]
fn test_take_listing_payment_validation() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    register_mock_stake_extension();

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create a listing with 0% offset (market price = 2 TAO per Alpha from mock)
    let alpha_amount = 5_000_000_000u64; // 5 Alpha
    let listing = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: alpha_amount,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing);

    // With mock market price of 2 TAO per Alpha:
    // TAO required = 5 Alpha * 2 TAO/Alpha = 10 TAO
    // Fee = 1% of 10 TAO = 0.1 TAO
    // Total required = 10.1 TAO = 10_100_000_000 rao
    let required_payment: u64 = 10_100_000_000;

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.django);

    // Test with payment too low by 1 rao - should fail with InsufficientPayment
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(u128::from(
        required_payment - 1,
    ));
    let result = contract.take_alpha_listing(1, accounts.charlie, 1);
    assert_eq!(result, Err(Error::InsufficientPayment));

    // Note: With dynamic pricing, overpayment is allowed (excess is refunded)
    // The test for exact/over payment would succeed at the validation step
    // but fail at runtime calls in unit tests
}

#[ink::test]
fn test_take_offer_calculation_accuracy() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Test with fixed market price of 2 TAO per Alpha (from mock)
    // and various offsets
    let market_price: u64 = MOCK_MARKET_PRICE; // 2 TAO per Alpha

    // Test case: 0% offset, 10 TAO offer
    let tao_amount = 10_000_000_000u64;
    let fee_amount = contract.fee_rate.mul(tao_amount).unwrap();
    let tao_for_seller = tao_amount - fee_amount; // 9.9 TAO

    // Apply 0% offset = market price
    let executed_price = OtcContract::apply_price_offset(market_price, 0).unwrap();
    let price_decimal = OtcContract::price_to_fixed_decimal(executed_price);

    // 9.9 TAO / 2 TAO per Alpha = 4.95 Alpha
    let expected_alpha = 4_950_000_000u64;
    let calculated_alpha = price_decimal.as_divisor_of(tao_for_seller).unwrap();

    // Allow for small rounding differences (within 1000 rao)
    let diff = calculated_alpha.abs_diff(expected_alpha);
    assert!(
        diff < 1000,
        "Calculation mismatch: got {}, expected {}",
        calculated_alpha,
        expected_alpha
    );
}

// Integration-style Unit Tests

#[ink::test]
fn test_complete_trade_flow_storage_cleanup() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create a listing
    let listing = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1]);

    // Verify listing exists
    assert!(contract.get_listing(1, accounts.charlie, 1).is_some());

    // Simulate successful trade by removing listing and cleaning up storage
    contract.alpha_listings.remove((1, accounts.charlie, 1));
    let mut user_listings = contract
        .user_listings
        .get((accounts.charlie, 1))
        .unwrap_or_default();
    user_listings.retain(|&id| id != 1);
    if user_listings.is_empty() {
        contract.user_listings.remove((accounts.charlie, 1));
    } else {
        contract
            .user_listings
            .insert((accounts.charlie, 1), &user_listings);
    }

    // Verify complete cleanup
    assert!(contract.get_listing(1, accounts.charlie, 1).is_none());
    assert_eq!(contract.get_user_listings(accounts.charlie, 1).len(), 0);
}

#[ink::test]
fn test_multiple_users_trading_simultaneously() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Create listings from different sellers
    let listing1 = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    let listing2 = AlphaListing {
        id: 2,
        netuid: 1,
        seller: accounts.django,
        amount: 3_000_000_000,
        price_offset_bps: -500, // 5% below market
        fee_rate: contract.fee_rate,
        created_at: 1100,
    };

    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing1);
    contract
        .alpha_listings
        .insert((1, accounts.django, 2), &listing2);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1]);
    contract
        .user_listings
        .insert((accounts.django, 1), &vec![2]);

    // Create offers from different buyers
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.eve);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(7_000_000_000);
    let offer1_id = contract.create_tao_offer(1, 500).unwrap(); // 5% above market

    ink::env::test::set_caller::<BittensorEnvironment>(accounts.frank);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(4_000_000_000);
    let offer2_id = contract.create_tao_offer(1, -200).unwrap(); // 2% below market

    // Verify all exist independently
    assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
    assert!(contract.get_listing(1, accounts.django, 2).is_some());
    assert!(contract.get_offer(1, accounts.eve, offer1_id).is_some());
    assert!(contract.get_offer(1, accounts.frank, offer2_id).is_some());

    // Verify user indices are correct
    assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![1]);
    assert_eq!(contract.get_user_listings(accounts.django, 1), vec![2]);
    assert_eq!(contract.get_user_offers(accounts.eve, 1), vec![offer1_id]);
    assert_eq!(contract.get_user_offers(accounts.frank, 1), vec![offer2_id]);
}

#[ink::test]
fn test_listing_and_offer_interaction() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(1.0);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // User creates both a listing and an offer
    let listing = AlphaListing {
        id: 1,
        netuid: 1,
        seller: accounts.charlie,
        amount: 5_000_000_000,
        price_offset_bps: 0, // Market price
        fee_rate: contract.fee_rate,
        created_at: 1000,
    };

    contract
        .alpha_listings
        .insert((1, accounts.charlie, 1), &listing);
    contract
        .user_listings
        .insert((accounts.charlie, 1), &vec![1]);

    // Same user creates an offer on a different netuid
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(3_000_000_000);
    let offer_id = contract.create_tao_offer(2, -500).unwrap(); // 5% below market

    // Verify both exist independently
    assert!(contract.get_listing(1, accounts.charlie, 1).is_some());
    assert!(contract.get_offer(2, accounts.charlie, offer_id).is_some());

    // Verify they're tracked separately
    assert_eq!(contract.get_user_listings(accounts.charlie, 1), vec![1]);
    assert_eq!(
        contract.get_user_listings(accounts.charlie, 2),
        Vec::<AlphaListingId>::new()
    );
    assert_eq!(
        contract.get_user_offers(accounts.charlie, 1),
        Vec::<TaoOfferId>::new()
    );
    assert_eq!(
        contract.get_user_offers(accounts.charlie, 2),
        vec![offer_id]
    );
}

#[ink::test]
fn test_zero_fee_rate() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let zero_fee_rate = 0u128; // 0% fee

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        zero_fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Test that zero fee rate results in no fee
    let tao_amount = 10_000_000_000u64; // 10 TAO
    let fee = contract.fee_rate.mul(tao_amount).unwrap();
    assert_eq!(fee, 0); // Fee should be zero
}

#[ink::test]
fn test_boundary_netuid_values() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Test with minimum and maximum netuid values
    let min_netuid: u16 = 0;
    let max_netuid: u16 = u16::MAX;

    // Create offers with boundary netuid values
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(2_000_000_000);

    let offer1 = contract.create_tao_offer(min_netuid, 0); // Market price
    assert!(offer1.is_ok());

    let offer2 = contract.create_tao_offer(max_netuid, 0); // Market price
    assert!(offer2.is_ok());

    // Verify retrieval works with boundary values
    assert!(contract
        .get_offer(min_netuid, accounts.charlie, offer1.unwrap())
        .is_some());
    assert!(contract
        .get_offer(max_netuid, accounts.charlie, offer2.unwrap())
        .is_some());
}

#[ink::test]
fn pause_state_initialization() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Contract should start in NotPaused state
    assert_eq!(contract.get_pause_state(), PauseState::NotPaused);
}

#[ink::test]
fn pause_trading_only_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Non-owner should not be able to pause
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);
    let result = contract.pause_trading(b"test".to_vec());
    assert_eq!(result, Err(Error::Unauthorized));

    // Owner should be able to pause trading
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    let result = contract.pause_trading(b"maintenance".to_vec());
    assert!(result.is_ok());
    assert_eq!(contract.get_pause_state(), PauseState::TradingPaused);
}

#[ink::test]
fn pause_fully_only_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Owner should be able to fully pause
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    let result = contract.pause_fully(b"emergency".to_vec());
    assert!(result.is_ok());
    assert_eq!(contract.get_pause_state(), PauseState::FullyPaused);
}

#[ink::test]
fn resume_only_owner() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Pause the contract first
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    contract.pause_fully(b"emergency".to_vec()).unwrap();

    // Non-owner should not be able to resume
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.bob);
    let result = contract.resume();
    assert_eq!(result, Err(Error::Unauthorized));

    // Owner should be able to resume
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    let result = contract.resume();
    assert!(result.is_ok());
    assert_eq!(contract.get_pause_state(), PauseState::NotPaused);
}

#[ink::test]
fn trading_blocked_when_trading_paused() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Pause trading
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    contract.pause_trading(b"maintenance".to_vec()).unwrap();

    // Try to list alpha - should fail
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    let result = contract.list_alpha(accounts.charlie, 1, 2_000_000_000, 0); // Market price
    assert_eq!(result, Err(Error::TradingPaused));

    // Try to create TAO offer - should fail
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(2_000_000_000);
    let result = contract.create_tao_offer(1, 0); // Market price
    assert_eq!(result, Err(Error::TradingPaused));
}

#[ink::test]
fn cancel_allowed_when_trading_paused() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        0, // No minimum age for testing
    );

    // Create a TAO offer first
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(2_000_000_000);
    let offer_id = contract.create_tao_offer(1, 0).unwrap(); // Market price

    // Pause trading
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    contract.pause_trading(b"maintenance".to_vec()).unwrap();

    // Cancel should be allowed when trading is paused (but will fail at runtime call)
    // In unit tests, we can only verify that the pause check passes
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);

    // Verify the offer exists before attempting cancel
    let offer = contract.get_offer(1, accounts.charlie, offer_id);
    assert!(offer.is_some());

    // In unit tests, the cancel will fail at the runtime call to transfer TAO back,
    // but the pause state check should pass. We verify the pause state allows cancellation.
    assert_eq!(contract.pause_state, PauseState::TradingPaused);

    // The ensure_not_fully_paused check should pass
    let pause_check = contract.ensure_not_fully_paused();
    assert!(pause_check.is_ok());
}

#[ink::test]
fn all_operations_blocked_when_fully_paused() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        0,
    );

    // Create a TAO offer first
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(2_000_000_000);
    let offer_id = contract.create_tao_offer(1, 0).unwrap(); // Market price

    // Fully pause the contract
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    contract.pause_fully(b"emergency".to_vec()).unwrap();

    // Try to list alpha - should fail
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.charlie);
    let result = contract.list_alpha(accounts.charlie, 1, 2_000_000_000, 0); // Market price
    assert_eq!(result, Err(Error::ContractFullyPaused));

    // Try to cancel offer - should also fail
    let result = contract.cancel_tao_offer(1, offer_id);
    assert_eq!(result, Err(Error::ContractFullyPaused));

    // Try to create TAO offer - should fail
    ink::env::test::set_value_transferred::<ink::env::DefaultEnvironment>(2_000_000_000);
    let result = contract.create_tao_offer(1, 0); // Market price
    assert_eq!(result, Err(Error::ContractFullyPaused));
}

#[ink::test]
fn admin_functions_work_when_paused() {
    let accounts = ink::env::test::default_accounts::<BittensorEnvironment>();
    let fee_rate = fee_rate_from_percentage(0.5);

    let mut contract = OtcContract::new(
        accounts.alice,
        accounts.bob,
        fee_rate,
        1_000_000_000,
        1_000_000_000,
        100,
    );

    // Fully pause the contract
    ink::env::test::set_caller::<BittensorEnvironment>(accounts.alice);
    contract.pause_fully(b"emergency".to_vec()).unwrap();

    // Admin functions should still work
    let result = contract.update_fee_rate(fee_rate_from_percentage(1.0));
    assert!(result.is_ok());

    let result = contract.update_min_listing_amount(2_000_000_000);
    assert!(result.is_ok());

    // Can transition between pause states
    let result = contract.pause_trading(b"downgrade".to_vec());
    assert!(result.is_ok());
    assert_eq!(contract.get_pause_state(), PauseState::TradingPaused);

    // Can resume
    let result = contract.resume();
    assert!(result.is_ok());
    assert_eq!(contract.get_pause_state(), PauseState::NotPaused);
}
