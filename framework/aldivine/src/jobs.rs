//! Aldivine Framework — jobs & organizations.
//!
//! Jobs are server-defined employment tiers (police, mechanic, ...). Each job
//! has grades; a grade carries a rank order, a label, and a salary. Duty
//! state is tracked separately from membership so an off-duty officer does
//! not accrue salary.

use std::collections::HashMap;

use ald_core::AldivinePlayerId;

#[derive(Debug, Clone)]
pub struct Grade {
    pub grade: u32,
    pub name: String,
    /// Higher rank = more senior. Used for permission inheritance.
    pub rank: u32,
    pub salary: u64,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub name: String,
    pub label: String,
    pub grades: Vec<Grade>,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("job '{0}' does not exist")]
    UnknownJob(String),
    #[error("grade {grade} does not exist in job '{job}'")]
    UnknownGrade { job: String, grade: u32 },
    #[error("player is not employed as '{0}'")]
    NotEmployed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DutyState {
    OnDuty,
    OffDuty,
}

#[derive(Debug, Clone)]
pub struct Employment {
    pub job: String,
    pub grade: u32,
    pub duty: DutyState,
}

/// Job registry + per-player employment.
#[derive(Debug, Default)]
pub struct JobService {
    jobs: HashMap<String, Job>,
    employment: HashMap<AldivinePlayerId, Employment>,
}

impl JobService {
    pub fn new() -> Self {
        JobService::default()
    }

    pub fn register(&mut self, job: Job) -> Result<(), JobError> {
        let mut grades = job.grades.clone();
        grades.sort_by_key(|g| g.grade);
        for (i, g) in grades.iter().enumerate() {
            if g.grade != i as u32 {
                return Err(JobError::UnknownGrade { job: job.name.clone(), grade: g.grade });
            }
        }
        self.jobs.insert(job.name.clone(), Job { grades, ..job });
        Ok(())
    }

    /// Employ or promote a player. The grade must exist in the job.
    pub fn set(&mut self, player: AldivinePlayerId, job: &str, grade: u32) -> Result<(), JobError> {
        let exists = self.jobs.contains_key(job);
        if !exists {
            return Err(JobError::UnknownJob(job.to_string()));
        }
        if !self.jobs[job].grades.iter().any(|g| g.grade == grade) {
            return Err(JobError::UnknownGrade { job: job.to_string(), grade });
        }
        self.employment.insert(player, Employment { job: job.to_string(), grade, duty: DutyState::OffDuty });
        Ok(())
    }

    pub fn fire(&mut self, player: AldivinePlayerId) -> Result<(), JobError> {
        match self.employment.remove(&player) {
            Some(_) => Ok(()),
            None => Err(JobError::NotEmployed("unemployed".into())),
        }
    }

    pub fn set_duty(&mut self, player: AldivinePlayerId, duty: DutyState) -> Result<(), JobError> {
        let e = self.employment.get_mut(&player).ok_or_else(|| JobError::NotEmployed("unemployed".into()))?;
        e.duty = duty;
        Ok(())
    }

    pub fn employment(&self, player: &AldivinePlayerId) -> Option<&Employment> {
        self.employment.get(player)
    }

    /// Salary for a player's current grade, or zero if unemployed/off-duty.
    pub fn salary(&self, player: &AldivinePlayerId) -> u64 {
        match self.employment.get(player) {
            Some(e) if e.duty == DutyState::OnDuty => self
                .jobs
                .get(&e.job)
                .and_then(|j| j.grades.iter().find(|g| g.grade == e.grade))
                .map(|g| g.salary)
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Grade rank for permission inheritance (higher = more authority).
    pub fn rank(&self, player: &AldivinePlayerId) -> u32 {
        self.employment
            .get(player)
            .and_then(|e| self.jobs.get(&e.job).and_then(|j| j.grades.iter().find(|g| g.grade == e.grade)))
            .map(|g| g.rank)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn police() -> Job {
        Job {
            name: "police".into(),
            label: "LSPD".into(),
            grades: vec![
                Grade { grade: 0, name: "Recruit".into(), rank: 1, salary: 100 },
                Grade { grade: 1, name: "Officer".into(), rank: 2, salary: 150 },
                Grade { grade: 2, name: "Sergeant".into(), rank: 3, salary: 250 },
            ],
        }
    }

    #[test]
    fn employ_and_salary() {
        let mut s = JobService::new();
        s.register(police()).unwrap();
        let p = AldivinePlayerId::new();
        s.set(p, "police", 1).unwrap();
        assert_eq!(s.salary(&p), 0); // off duty
        s.set_duty(p, DutyState::OnDuty).unwrap();
        assert_eq!(s.salary(&p), 150);
    }

    #[test]
    fn unknown_job_rejected() {
        let mut s = JobService::new();
        let p = AldivinePlayerId::new();
        assert!(matches!(s.set(p, "nonexistent", 0), Err(JobError::UnknownJob(_))));
    }

    #[test]
    fn unknown_grade_rejected() {
        let mut s = JobService::new();
        s.register(police()).unwrap();
        let p = AldivinePlayerId::new();
        assert!(matches!(s.set(p, "police", 9), Err(JobError::UnknownGrade { .. })));
    }

    #[test]
    fn fire_unemployed_fails() {
        let mut s = JobService::new();
        assert!(s.fire(AldivinePlayerId::new()).is_err());
    }

    #[test]
    fn rank_increases_with_grade() {
        let mut s = JobService::new();
        s.register(police()).unwrap();
        let p = AldivinePlayerId::new();
        s.set(p, "police", 2).unwrap();
        assert_eq!(s.rank(&p), 3);
    }

    #[test]
    fn grade_gap_rejected() {
        let mut s = JobService::new();
        // Grades must be contiguous starting at 0.
        let bad = Job {
            name: "bad".into(),
            label: "Bad".into(),
            grades: vec![
                Grade { grade: 0, name: "A".into(), rank: 1, salary: 1 },
                Grade { grade: 2, name: "C".into(), rank: 3, salary: 3 },
            ],
        };
        assert!(s.register(bad).is_err());
    }
}
