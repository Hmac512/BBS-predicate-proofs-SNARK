use ark_ff::{Field, PrimeField};
use ark_r1cs_std::alloc::{AllocVar, AllocationMode};
use ark_r1cs_std::fields::fp::FpVar;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use std::cmp::Ordering;

// NOTE: For range check, the following circuits assume that the numbers are of same size as field
// elements which might not always be true in practice. If the upper bound on the byte-size of the numbers
// is known, then the no. of constraints in the circuit can be reduced.

/// Enforce min < value < max
#[derive(Clone)]
pub struct BoundCheckCircuit<F: Field> {
    min: Option<F>,
    max: Option<F>,
    value: Option<F>,
}

impl<ConstraintF: PrimeField> ConstraintSynthesizer<ConstraintF>
    for BoundCheckCircuit<ConstraintF>
{
    fn generate_constraints(
        self,
        cs: ConstraintSystemRef<ConstraintF>,
    ) -> Result<(), SynthesisError> {
        let val = FpVar::new_variable(
            cs.clone(),
            || self.value.ok_or(SynthesisError::AssignmentMissing),
            AllocationMode::Witness,
        )?;

        let min = FpVar::new_variable(
            cs.clone(),
            || self.min.ok_or(SynthesisError::AssignmentMissing),
            AllocationMode::Input,
        )?;
        let max = FpVar::new_variable(
            cs.clone(),
            || self.max.ok_or(SynthesisError::AssignmentMissing),
            AllocationMode::Input,
        )?;

        // val strictly less than max, i.e. val < max and val != max
        val.enforce_cmp(&max, Ordering::Less, false)?;
        // val strictly greater than max, i.e. val > min and val != min
        val.enforce_cmp(&min, Ordering::Greater, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;
    use ark_bls12_381::Bls12_381;
    use ark_std::{
        collections::{BTreeMap, BTreeSet},
        rand::{rngs::StdRng, SeedableRng},
        UniformRand,
    };
    use legogroth16::{
        create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof,
        verify_witness_commitment,
    };
    use proof_system::prelude::{
        EqualWitnesses, MetaStatement, MetaStatements, ProofSpec, Statement, Statements, Witness,
        WitnessRef, Witnesses,
    };

    #[test]
    fn bound_check_message() {
        // Prover has a BBS+ signature and he wants to prove that one of the signed message satisfies `min < message < max`
        // on public `min` and `max` but hiding the message. This will be useful in doing proof of age in a range

        let mut rng = StdRng::seed_from_u64(0u64);
        // Prover has the BBS+ signature
        let message_count = 10;
        let (messages, sig_params, bls_keypair, bbs_sig) = sig_setup(&mut rng, message_count);
        bbs_sig
            .verify(&messages, &bls_keypair.public_key, &sig_params)
            .unwrap();

        // Only 1 witness that is the message whose bounds need to proved is committed
        let commit_witness_count = 1;

        let arithmetic_circuit = BoundCheckCircuit::<Fr> {
            min: None,
            max: None,
            value: None,
        };
        let params = generate_random_parameters::<Bls12_381, _, _>(
            arithmetic_circuit,
            commit_witness_count,
            &mut rng,
        )
        .unwrap();

        let pvk = prepare_verifying_key(&params.vk);

        // Create commitment randomness
        let v = Fr::rand(&mut rng);

        // Message whose bounds need to be proved, i.e. `min < val < max` needs to be proved
        let msg_idx = 4;
        let msg_val = messages[msg_idx].clone();

        let min = Fr::from(100u64);
        let max = Fr::from(107u64);

        let arithmetic_circuit = BoundCheckCircuit {
            min: Some(min),
            max: Some(max),
            value: Some(msg_val),
        };

        // Prover creates LegoGroth16 proof
        let zk_snark = create_random_proof(arithmetic_circuit, v, &params, &mut rng).unwrap();

        // This is not done by the verifier but the prover as safety check that the commitment is correct
        verify_witness_commitment(&params.vk, &zk_snark, 2, &[msg_val], &v).unwrap();
        assert!(verify_witness_commitment(&params.vk, &zk_snark, 1, &[msg_val], &v).is_err());
        assert!(verify_witness_commitment(&params.vk, &zk_snark, 3, &[msg_val], &v).is_err());
        assert!(
            verify_witness_commitment(&params.vk, &zk_snark, 2, &[Fr::from(106u64)], &v).is_err()
        );

        // The bases and commitment opening
        let bases = vec![params.vk.gamma_abc_g1[1 + 2], params.vk.eta_gamma_inv_g1];
        let committed = vec![msg_val, v];

        // Since both prover and verifier know the public inputs, they can independently get the commitment to the witnesses
        let commitment_to_witness = zk_snark.d;

        // Prove the equality of message in the BBS+ signature and `commitment_to_witness`
        let mut statements = Statements::new();
        statements.add(Statement::PoKBBSSignatureG1(PoKSignatureBBSG1Stmt {
            params: sig_params.clone(),
            public_key: bls_keypair.public_key.clone(),
            revealed_messages: BTreeMap::new(),
        }));
        statements.add(Statement::PedersenCommitment(PedersenCommitmentStmt {
            bases: bases.clone(),
            commitment: commitment_to_witness.clone(),
        }));

        let mut meta_statements = MetaStatements::new();
        meta_statements.add(MetaStatement::WitnessEquality(EqualWitnesses(
            vec![(0, msg_idx), (1, 0)] // 0th statement's `m_idx`th witness is equal to 1st statement's 0th witness
                .into_iter()
                .collect::<BTreeSet<WitnessRef>>(),
        )));

        let proof_spec = ProofSpec {
            statements: statements.clone(),
            meta_statements: meta_statements.clone(),
            context: None,
        };

        let mut witnesses = Witnesses::new();
        witnesses.add(PoKSignatureBBSG1Wit::new_as_witness(
            bbs_sig.clone(),
            messages
                .clone()
                .into_iter()
                .enumerate()
                .map(|t| t)
                .collect(),
        ));
        witnesses.add(Witness::PedersenCommitment(committed));

        let proof = ProofG1::new(&mut rng, proof_spec.clone(), witnesses.clone(), None).unwrap();

        // verifies Groth16 proof
        verify_proof(&pvk, &zk_snark, &[min, max]).unwrap();

        proof.verify(proof_spec, None).unwrap();
    }
}
