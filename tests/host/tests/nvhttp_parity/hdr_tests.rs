//! HDR mode query parsing parity test extracted from `parity_tests`.
use butterpollo_media_policy::{rtsp::Negotiated, stream_policy::infer_sdr_10bit};
use butterpollo_session::session::Role;
use butterpollo_stream::nvhttp::{Args, build_launch_session};
use butterpollo_stream::state::test_support::Fixture;

#[test]
fn only_hdr_mode_zero_makes_a_ten_bit_format_sdr() {
    let f = Fixture::new();
    let client = f.client(0);
    let sdp = b"a=x-nv-vqos[0].bitStreamFormat:1\na=x-nv-video[0].dynamicRangeMode:1\n";
    // /launch and /resume build the session from the same query.
    for (query, sdr) in [(None, false), (Some("0"), true), (Some("1"), false)] {
        let args: Args = query
            .into_iter()
            .map(|v| ("hdrMode".to_string(), v.to_string()))
            .collect();
        let launch = build_launch_session(
            &f.connection(true),
            &args,
            client.clone(),
            1,
            Role::Stream,
            [0; 16],
            1,
        );
        let mut negotiated = Negotiated::from_sdp(sdp).unwrap();
        assert!(negotiated.hdr);
        infer_sdr_10bit(&mut negotiated, launch.ten_bit_sdr_requested());
        assert_eq!(
            (negotiated.hdr, negotiated.sdr_10bit),
            (!sdr, sdr),
            "hdrMode={query:?}"
        );
        assert!(negotiated.ten_bit());
    }
}
